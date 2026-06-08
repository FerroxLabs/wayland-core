//! Input event polling and translation.
//!
//! This module owns the crossterm event side of the render loop. It polls
//! for terminal events with a timeout and translates everything the router
//! cares about into a flat batch of [`InputEvent`]s.
//!
//! ## Why a batch, not one event per tick
//!
//! The render loop runs at ~30fps and the router consumes input one
//! [`KeyEvent`] at a time. If [`poll_input`] surfaced a single event per
//! call, a paste — which a terminal delivers as a fast burst of dozens to
//! thousands of key events (or one bracketed-paste blob) — would drain at
//! one keystroke per 33ms frame: pasting 3 000 characters would take 100
//! seconds and *look* like a hang. [`poll_input`] instead drains every
//! event already buffered in the terminal in one call, so a paste of any
//! size is consumed in a single tick. The drain is bounded
//! ([`MAX_DRAIN`]) so a pathological input stream can never starve the
//! render: input past the cap is left in the OS buffer for the next tick.
//!
//! ## Bracketed paste
//!
//! With bracketed paste enabled (see `mod.rs`'s terminal setup), a paste
//! arrives as one [`Event::Paste`] carrying the whole blob. [`poll_input`]
//! wraps it into a single [`InputEvent::PastedBlock`] and leaves all
//! decomposition (including newline handling) to the surface that consumes
//! it. Newlines inside a pasted blob must **not** auto-submit a turn — only
//! an explicit `Enter` outside a paste block should do that. A surface
//! that receives a `PastedBlock` inserts the text verbatim.

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent,
};

/// Upper bound on events drained from the terminal in a single
/// [`poll_input`] call. A paste of normal size is far below this; the cap
/// only exists so a degenerate, never-ending input stream cannot hold the
/// render loop hostage. Anything past the cap stays in the OS input buffer
/// and is picked up on the next tick.
const MAX_DRAIN: usize = 8_192;

/// A translated terminal event the render loop acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// A key press (repeats and releases are already filtered out).
    Key(KeyEvent),
    /// A bracketed-paste blob, delivered as one atomic string. The whole
    /// pasted text (including any embedded newlines) is wrapped here so
    /// the composer can insert it verbatim — newlines inside a paste must
    /// NOT auto-submit a turn. Only an `Enter` the user presses *outside*
    /// a bracketed-paste block is a submit.
    PastedBlock(String),
    /// The terminal was resized to `cols`×`rows`. The render loop redraws
    /// on the next tick regardless; this is surfaced so a resize *during*
    /// a stream is observed promptly rather than waiting for the tick.
    Resize {
        /// New terminal width in columns.
        cols: u16,
        /// New terminal height in rows.
        rows: u16,
    },
    /// A mouse event — scroll-wheel, click, or motion. Routed to the
    /// focused surface's `handle_mouse`. Mouse capture is enabled by
    /// `tui::run` (W0.1) and disabled on shutdown by the same path; if a
    /// terminal silently ignores `EnableMouseCapture` no event arrives.
    Mouse(MouseEvent),
    /// The terminal gained input focus. W1 (SPEC §1A): drives
    /// `AnimationClock::set_paused(false)` so animation ticks resume when
    /// the user returns to the window.
    FocusGained,
    /// The terminal lost input focus. W1 (SPEC §1A): drives
    /// `AnimationClock::set_paused(true)` so animation ticks stop while the
    /// user is in another window. A terminal that never emits focus events
    /// simply never pauses on blur — no dead-end (the idle-dwell win holds).
    FocusLost,
}

/// Poll for terminal input, waiting up to `timeout` for the first event,
/// then draining every other event already buffered without blocking.
///
/// Returns the batch in arrival order. An empty `Vec` means the timeout
/// elapsed with no input (the loop still redraws on its tick). Key
/// repeats/releases are filtered out; a [`Event::Paste`] is wrapped into a
/// single [`InputEvent::PastedBlock`]; an [`Event::Mouse`] is wrapped into
/// [`InputEvent::Mouse`] (scroll-wheel drives transcript scrollback in the
/// workspace, D2/v0.9.0); focus changes surface as
/// [`InputEvent::FocusGained`]/[`InputEvent::FocusLost`] (W1, drives the
/// animation-clock pause/resume).
pub fn poll_input(timeout: Duration) -> Result<Vec<InputEvent>> {
    let mut out = Vec::new();

    // Block up to `timeout` for the first event so the loop sleeps when
    // idle; bail straight away on a timeout.
    if !event::poll(timeout)? {
        return Ok(out);
    }
    translate_into(event::read()?, &mut out);

    // Drain everything else the terminal has already buffered — a paste,
    // a held key, a resize storm — without blocking, so the whole burst
    // lands in this one tick. `MAX_DRAIN` bounds the worst case.
    while out.len() < MAX_DRAIN && event::poll(Duration::ZERO)? {
        translate_into(event::read()?, &mut out);
    }
    Ok(out)
}

/// Translate one raw crossterm [`Event`] into zero or more [`InputEvent`]s,
/// appended to `out`. Key repeat/release translate to nothing; mouse events
/// surface as [`InputEvent::Mouse`]; focus changes surface as
/// [`InputEvent::FocusGained`]/[`InputEvent::FocusLost`] (W1).
fn translate_into(ev: Event, out: &mut Vec<InputEvent>) {
    match ev {
        // Only key *presses* reach the router — releases/repeats (which
        // some terminals, e.g. Windows, emit) would double-fire keys.
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            out.push(InputEvent::Key(key));
        }
        // A bracketed-paste blob: wrap the whole text into a single
        // `PastedBlock` so the composer can insert it verbatim.  Newlines
        // inside the paste must NOT auto-submit a turn — only an explicit
        // Enter the user presses *outside* a paste block does that.
        Event::Paste(text) => out.push(InputEvent::PastedBlock(text)),
        Event::Resize(cols, rows) => out.push(InputEvent::Resize { cols, rows }),
        // Mouse events — scroll-wheel ticks drive transcript scrollback in
        // the workspace (D2/v0.9.0). Mouse capture is enabled by `tui::run`;
        // a terminal that ignores the escape simply emits no `Event::Mouse`.
        Event::Mouse(m) => out.push(InputEvent::Mouse(m)),
        // W1 (SPEC §1A): focus changes drive the AnimationClock pause/
        // resume so animation ticks stop while the terminal is blurred.
        // Surfaced (no longer dropped); the run_loop maps them onto
        // `clock.set_paused(...)`.
        Event::FocusGained => out.push(InputEvent::FocusGained),
        Event::FocusLost => out.push(InputEvent::FocusLost),
        // Key releases/repeats — drained off the queue so they cannot back
        // up, but not acted on.
        _ => {}
    }
}

/// A modifier-free key press for `code` — the shape a typed/pasted key
/// takes once it reaches the router.
fn plain(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `KeyEvent`s in a batch, in order — discards `Resize`s and
    /// `PastedBlock`s.
    fn keys(batch: &[InputEvent]) -> Vec<KeyCode> {
        batch
            .iter()
            .filter_map(|e| match e {
                InputEvent::Key(k) => Some(k.code),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn paste_event_produces_a_single_pasted_block() {
        // F-041: a bracketed-paste blob — including one with embedded
        // newlines — must translate to exactly ONE `PastedBlock`, never to
        // a sequence of `Key(Enter)` presses.  Embedded newlines must not
        // auto-submit a turn.
        let mut out = Vec::new();
        translate_into(Event::Paste("line1\nline2\nline3".to_string()), &mut out);
        assert_eq!(out.len(), 1, "a paste must yield exactly one event");
        assert_eq!(
            out[0],
            InputEvent::PastedBlock("line1\nline2\nline3".to_string()),
            "the paste event must be a PastedBlock carrying the full text"
        );
        // Critically: no KeyCode::Enter in the output.
        assert_eq!(
            keys(&out),
            vec![],
            "no Enter keys must be emitted for a paste"
        );
    }

    #[test]
    fn paste_event_carries_the_full_blob_including_crlf() {
        // CRLF-style pastes are also wrapped whole — the composer handles
        // normalisation if it wants to.
        let mut out = Vec::new();
        translate_into(Event::Paste("a\r\nb".to_string()), &mut out);
        assert_eq!(out[0], InputEvent::PastedBlock("a\r\nb".to_string()),);
    }

    #[test]
    fn large_paste_is_one_event_not_many() {
        // A 50k-character paste must produce exactly one `PastedBlock`
        // event — not 50k individual key events.
        let big: String = "x".repeat(50_000);
        let mut out = Vec::new();
        translate_into(Event::Paste(big.clone()), &mut out);
        assert_eq!(out.len(), 1, "large paste must still be one event");
        assert_eq!(out[0], InputEvent::PastedBlock(big));
    }

    #[test]
    fn translate_keeps_only_key_presses() {
        let press = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press
        };
        let mut out = Vec::new();
        translate_into(Event::Key(press), &mut out);
        translate_into(Event::Key(release), &mut out);
        // The release is dropped; only the press survives.
        assert_eq!(keys(&out), vec![KeyCode::Char('a')]);
    }

    #[test]
    fn translate_surfaces_a_resize() {
        let mut out = Vec::new();
        translate_into(Event::Resize(120, 40), &mut out);
        assert_eq!(
            out,
            vec![InputEvent::Resize {
                cols: 120,
                rows: 40
            }]
        );
    }

    #[test]
    fn focus_events_are_surfaced_not_dropped() {
        // W1 (SPEC §1A): focus changes used to be discarded; they now
        // surface so the run_loop can pause/resume the animation clock on
        // blur/focus. Order is preserved.
        let mut out = Vec::new();
        translate_into(Event::FocusGained, &mut out);
        translate_into(Event::FocusLost, &mut out);
        assert_eq!(out, vec![InputEvent::FocusGained, InputEvent::FocusLost]);
    }

    #[test]
    fn key_releases_still_translate_to_nothing() {
        // The W1 focus change must not have re-admitted key releases — the
        // catch-all still drops them so a held key cannot double-fire.
        let release = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        let mut out = Vec::new();
        translate_into(Event::Key(release), &mut out);
        assert!(out.is_empty(), "key releases must still be dropped");
    }

    #[test]
    fn max_drain_is_a_meaningful_bound() {
        // The drain cap must be large enough not to chop a realistic paste
        // mid-stream, yet finite so the loop cannot be starved. `MAX_DRAIN`
        // is a const, so the bound is checked at compile time.
        const _: () = assert!(MAX_DRAIN >= 4_096);
        const _: () = assert!(MAX_DRAIN < usize::MAX);
    }
}
