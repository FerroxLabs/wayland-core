//! Streaming-status widget — the animated "working" line that replaced the
//! static spinner-plus-"working" placeholder.
//!
//! v0.9.0 W3 D3. Renders a single composer-row strip with five components:
//!
//! ```text
//! ✻ Leavening… (1m 23s · ↑ 439 tokens · calling web_search · tip: Mouse-wheel scrolls)
//! ^ ^         ^ ^      ^ ^             ^ ^                  ^ ^
//! | |         | |      | |             | |                  | tip body
//! | |         | |      | |             | phase label        |
//! | |         | |      | token counter |                    |
//! | |         | elapsed clock          |                    |
//! | verb (sampled once per turn)       |                    |
//! 4-frame animated symbol              |                    |
//! ```
//!
//! v0.9.2 W6 (SPEC §4 + §3 #6): the verb is now a SINGLE pick per turn
//! (`streaming::pick_turn_verb(session.turn_verb_seed)`) — constant for the
//! whole turn, not a time-based rotation. The animated symbol's color also
//! lerps from `theme.orange` toward `theme.error` after ~3s of no token
//! deltas (a "stalled" signal) and recovers to orange when deltas resume.
//!
//! Every piece is a deterministic function of [`SessionView`] — no widget-
//! local clock, no thread, no allocation per frame beyond the [`Line`] the
//! caller renders. The phase enum is locked (see `app::StreamingPhase`);
//! verbs + tips are local to this module.
//!
//! CPU posture: the caller decides whether to render at all (it checks
//! `phase != Idle` first). When called, we compute one [`Line`] per tick
//! against the already-published `app.frame_tick` — there is no per-widget
//! tick state. The render loop already runs at ~30fps for the braille
//! spinner; we ride that tick.

use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::app::{SessionView, StreamingPhase};
use crate::tui::theme::Theme;

/// The 4-frame symbol animation. One frame ≈ 250ms at ~30fps the loop runs at.
const SYMBOL_FRAMES: [&str; 4] = ["✻", "⋆", "✦", "✷"];

/// Number of render ticks each symbol frame is held. The render loop ticks
/// ~30/s; holding 8 ticks per frame yields a ~4 cycles/s pulse — fast
/// enough to read as "alive", slow enough not to strobe.
const SYMBOL_TICKS_PER_FRAME: u64 = 8;

/// v0.9.2 W6 (SPEC §4): the verb is a SINGLE pick per turn, not a rotation.
/// The pool ([`crate::tui::streaming::SPINNER_VERBS`], ≥150 entries) and the
/// sampler ([`crate::tui::streaming::pick_turn_verb`]) live in the
/// `streaming` module; the per-turn seed lives on `SessionView::turn_verb_seed`
/// (set once at `StreamStart`). The old `VERBS`/`VERB_ROTATION_MILLIS`/
/// `pick_verb` time-based rotation was removed here.
///
/// ── v0.9.2 W6 stall lerp ──
/// After ~3s of no token deltas the spinner glyph reddens from
/// `theme.orange` toward `theme.error`, reaching full error at ~8s, and
/// recovers to orange when deltas resume.
const STALL_LERP_START_SECS: f64 = 3.0;
/// Stall window over which the glyph lerps fully from orange to error (so
/// full error is reached at `STALL_LERP_START_SECS + STALL_LERP_SPAN_SECS`).
const STALL_LERP_SPAN_SECS: f64 = 5.0;

/// Tips shown below the verb after a phase has held for >15s. Rotated
/// every [`TIP_ROTATION_SECS`] of elapsed phase time, deterministic from
/// `phase_started_at`.
const TIPS: [&str; 8] = [
    "Use /btw to ask a quick side question without interrupting",
    "Press Ctrl+E to expand tool details",
    "Mouse-wheel scrolls the transcript",
    "↓ jump to latest scrollback hint appears when scrolled up",
    "Approval modal opens for mutating tools — Y/N to decide",
    "Sources block at turn-end collects all cited URLs",
    "Press Ctrl+C twice to exit",
    "Type /doctor to check provider health",
];

/// Phase-hold threshold before tips begin appearing.
const TIP_AFTER_SECS: u64 = 15;

/// How long each tip holds before the rotation advances, in seconds.
const TIP_ROTATION_SECS: u64 = 30;

/// Build the animated streaming-status line for a session that has a
/// stream in flight. Caller must gate on `phase != Idle` before invoking.
///
/// `frame_tick` is the render loop's monotonic frame counter — the same
/// one the braille spinner reads (`App::frame_tick`).
pub fn render_streaming_status(
    session: &SessionView,
    frame_tick: u64,
    theme: &Theme,
) -> Line<'static> {
    render_streaming_status_at(session, frame_tick, Instant::now(), theme)
}

/// Pure form for testing — takes `now` as a parameter so tests can
/// synthesize specific elapsed-time windows.
pub fn render_streaming_status_at(
    session: &SessionView,
    frame_tick: u64,
    now: Instant,
    theme: &Theme,
) -> Line<'static> {
    // v0.9.1.2 F14: when the engine has parked a tool call waiting for
    // the user's approval, the working line MUST stop saying
    // "Brewing/Calling/Steeping" — those verbs imply work is happening,
    // but the actual state is "input required from you". Render the
    // dedicated awaiting-approval label instead, no verb rotation, no
    // spinner motion, in warn-yellow + bold so it reads as a status
    // change. `(+N more pending)` tail when a batch has stacked up.
    if let StreamingPhase::AwaitingApproval {
        tool,
        pending_count,
    } = &session.phase
    {
        let mut text = format!("⊘ Awaiting your approval: {tool}");
        if *pending_count > 1 {
            let extra = pending_count.saturating_sub(1);
            text.push_str(&format!(" (+{extra} more pending)"));
        }
        return Line::from(Span::styled(
            text,
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let symbol =
        SYMBOL_FRAMES[(frame_tick / SYMBOL_TICKS_PER_FRAME) as usize % SYMBOL_FRAMES.len()];

    let turn_elapsed = now.saturating_duration_since(session.turn_started_at);
    // v0.9.2 W6: single pick from the per-turn seed — constant for the turn.
    let verb = crate::tui::streaming::pick_turn_verb(session.turn_verb_seed);
    let elapsed = format_duration(turn_elapsed);
    let tokens = session.tokens_out;
    let phase_label = session.phase.display_label();

    let phase_elapsed = now.saturating_duration_since(session.phase_started_at);
    let tip = pick_tip(phase_elapsed);

    // Composition: symbol verb… (elapsed · ↑ tokens · phase[· tip])
    // v0.9.2 W6: the symbol color lerps orange→error after ~3s of no deltas.
    let glyph_color = stall_glyph_color(session, now, theme);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
    spans.push(Span::styled(
        format!("{symbol} "),
        Style::default().fg(glyph_color),
    ));
    spans.push(Span::styled(
        format!("{verb}… "),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    ));
    let mut detail = String::new();
    detail.push('(');
    detail.push_str(&elapsed);
    detail.push_str(" · ↑ ");
    detail.push_str(&tokens.to_string());
    detail.push_str(" tokens · ");
    detail.push_str(&phase_label);
    if let Some(tip_text) = tip {
        detail.push_str(" · ");
        detail.push_str(tip_text);
    }
    detail.push(')');
    spans.push(Span::styled(detail, Style::default().fg(theme.text_dim)));

    Line::from(spans)
}

/// v0.9.2 W6 (SPEC §3 #6): the spinner glyph color for a session, lerping
/// from `theme.orange` toward `theme.error` as the time since the last token
/// delta grows past [`STALL_LERP_START_SECS`]. Thin wrapper that computes the
/// stall duration from `session.last_delta_at` and delegates to
/// [`stall_lerp_color`].
///
/// `pub(crate)` so the streaming-status render path and tests can reuse it.
pub(crate) fn stall_glyph_color(session: &SessionView, now: Instant, theme: &Theme) -> Color {
    let stall = now.saturating_duration_since(session.last_delta_at);
    stall_lerp_color(stall, theme)
}

/// Pure RGB color-lerp for the stall signal: returns `theme.orange` for the
/// first [`STALL_LERP_START_SECS`], then interpolates orange→error across the
/// next [`STALL_LERP_SPAN_SECS`], clamping at `theme.error` thereafter.
///
/// Non-RGB themes (256-color / `NO_COLOR`) have no smooth lerp; the helper
/// snaps to `theme.error` once fully stalled and otherwise stays on
/// `theme.orange`, so the signal is still visible without a gradient.
pub(crate) fn stall_lerp_color(stall: Duration, theme: &Theme) -> Color {
    let secs = stall.as_secs_f64();
    if secs < STALL_LERP_START_SECS {
        return theme.orange;
    }
    let t = ((secs - STALL_LERP_START_SECS) / STALL_LERP_SPAN_SECS).clamp(0.0, 1.0);
    lerp_rgb(theme.orange, theme.error, t)
}

/// Linearly interpolate between two RGB colors by `t` in `[0, 1]`. Only
/// `Color::Rgb` pairs are blended — for any other variant (Indexed / Reset)
/// there is no continuous space, so we snap: `t < 1.0` keeps `a`, `t == 1.0`
/// yields `b`. This keeps the stall signal visible on non-truecolor themes
/// (it flips orange→error at full stall) without inventing a fake gradient.
fn lerp_rgb(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    if let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) {
        let lerp = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
        Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
    } else if t >= 1.0 {
        b
    } else {
        a
    }
}

/// Pick the tip for a phase that has been held `phase_elapsed`. Returns
/// `None` until the phase has held for [`TIP_AFTER_SECS`] so short turns
/// stay quiet; rotates every [`TIP_ROTATION_SECS`] thereafter.
fn pick_tip(phase_elapsed: Duration) -> Option<&'static str> {
    let secs = phase_elapsed.as_secs();
    if secs < TIP_AFTER_SECS {
        return None;
    }
    let idx = ((secs - TIP_AFTER_SECS) / TIP_ROTATION_SECS) as usize % TIPS.len();
    Some(TIPS[idx])
}

/// Format an elapsed duration as a short human-readable string:
/// `42s`, `1m 23s`, or `1h 5m`. No leading zeros and no padding so the
/// clock reads naturally as it grows.
pub fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    if total < 60 {
        format!("{total}s")
    } else if total < 3600 {
        let m = total / 60;
        let s = total % 60;
        format!("{m}m {s}s")
    } else {
        let h = total / 3600;
        let m = (total % 3600) / 60;
        format!("{h}h {m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Build a session pinned to `now` with a specific phase + token count.
    fn session_with(phase: StreamingPhase, tokens: u64) -> SessionView {
        SessionView {
            streaming_active: true,
            phase,
            tokens_out: tokens,
            ..Default::default()
        }
    }

    #[test]
    fn streaming_phase_display_label_matches_lock() {
        // Regression guard: if a contributor adds a variant or renames one
        // of the locked labels, this test fails before the cross-audit
        // sees the diff. The labels are part of the v0.9.0 W3 contract.
        assert_eq!(StreamingPhase::Idle.display_label(), "idle");
        assert_eq!(StreamingPhase::Thinking.display_label(), "thinking");
        assert_eq!(StreamingPhase::Drafting.display_label(), "drafting reply");
        assert_eq!(
            StreamingPhase::CallingTool("web_search".into()).display_label(),
            "calling web_search"
        );
        assert_eq!(
            StreamingPhase::RunningTool("Bash".into()).display_label(),
            "running Bash"
        );
        assert_eq!(StreamingPhase::WrappingUp.display_label(), "wrapping up");
        // v0.9.1.2 F14: awaiting-approval phase label is locked too.
        assert_eq!(
            StreamingPhase::AwaitingApproval {
                tool: "Write".into(),
                pending_count: 1
            }
            .display_label(),
            "awaiting approval: Write"
        );
    }

    #[test]
    fn elapsed_formats_correctly_under_minute_over_minute_over_hour() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(42)), "42s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59s");
        // 60s → 1m 0s (we don't suppress the seconds at the boundary).
        assert_eq!(format_duration(Duration::from_secs(60)), "1m 0s");
        assert_eq!(format_duration(Duration::from_secs(83)), "1m 23s");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59m 59s");
        // 1h 5m: hours+minutes form drops the trailing seconds.
        assert_eq!(format_duration(Duration::from_secs(3900)), "1h 5m");
        assert_eq!(format_duration(Duration::from_secs(7260)), "2h 1m");
    }

    // ── v0.9.2 W6 — single-pick verb + RGB color-lerp stall ──

    /// Pull the bolded verb word out of a rendered status line. The verb is
    /// always the SECOND span (`"{verb}… "`); we strip the trailing "… ".
    fn extract_verb(line: &Line<'static>) -> String {
        line.spans
            .get(1)
            .map(|s| {
                s.content
                    .as_ref()
                    .trim_end()
                    .trim_end_matches('…')
                    .to_string()
            })
            .unwrap_or_default()
    }

    #[test]
    fn verb_is_constant_across_frames_of_one_turn() {
        // v0.9.2 W6 (SPEC §4): no rotation. With a fixed turn_verb_seed the
        // verb word is identical across frames — only the spinner symbol
        // frame advances with frame_tick.
        let mut s = session_with(StreamingPhase::Drafting, 100);
        s.turn_verb_seed = 42;
        let now = Instant::now();
        s.turn_started_at = now;
        s.phase_started_at = now;
        s.last_delta_at = now;
        let theme = Theme::hearth();
        let l0 = render_streaming_status_at(&s, 0, now, &theme);
        let l9 = render_streaming_status_at(&s, 9, now, &theme);
        assert_eq!(extract_verb(&l0), extract_verb(&l9));
        // And it's a real pool entry.
        assert_eq!(extract_verb(&l0), crate::tui::streaming::pick_turn_verb(42));
    }

    #[test]
    fn verb_differs_across_turns_probabilistically() {
        // Two turns with different seeds (probabilistically) render different
        // verbs — the cross-turn-variation half of SPEC §4 acceptance.
        let theme = Theme::hearth();
        let now = Instant::now();
        let distinct: std::collections::HashSet<String> = (0..50u64)
            .map(|seed| {
                let mut s = session_with(StreamingPhase::Drafting, 0);
                s.turn_verb_seed = seed;
                s.turn_started_at = now;
                s.phase_started_at = now;
                s.last_delta_at = now;
                extract_verb(&render_streaming_status_at(&s, 0, now, &theme))
            })
            .collect();
        assert!(distinct.len() > 1, "verb should vary across turns");
    }

    #[test]
    fn glyph_color_is_orange_before_stall() {
        // <3s since last delta → pure brand orange, no lerp.
        let theme = Theme::hearth();
        assert_eq!(
            stall_lerp_color(Duration::from_secs(0), &theme),
            theme.orange
        );
        assert_eq!(
            stall_lerp_color(Duration::from_millis(2900), &theme),
            theme.orange
        );
    }

    #[test]
    fn glyph_color_lerps_toward_error_after_3s_stall() {
        // After 4s the lerp has moved off pure orange toward error red, but
        // not yet reached full error (the span is 3s..8s).
        let theme = Theme::hearth();
        let c4 = stall_lerp_color(Duration::from_secs(4), &theme);
        assert_ne!(c4, theme.orange, "should have left pure orange by 4s");
        assert_ne!(c4, theme.error, "should not be full error yet at 4s");
        // Intermediate value sits strictly between the two RGB endpoints on
        // the green channel (orange #ff6b35 g=0x6b → error #f87171 g=0x71).
        if let (Color::Rgb(_, og, _), Color::Rgb(_, eg, _), Color::Rgb(_, cg, _)) =
            (theme.orange, theme.error, c4)
        {
            let lo = og.min(eg);
            let hi = og.max(eg);
            assert!(cg >= lo && cg <= hi, "green channel out of lerp range");
        } else {
            panic!("hearth theme should be truecolor RGB");
        }
    }

    #[test]
    fn glyph_color_clamps_to_error_after_6s_stall() {
        // By 8s the lerp is complete; at 6s it is well past halfway. Past the
        // full span (≥8s) it clamps exactly to error.
        let theme = Theme::hearth();
        let c6 = stall_lerp_color(Duration::from_secs(6), &theme);
        assert_ne!(c6, theme.orange);
        // At and beyond the full span the color is exactly error red.
        assert_eq!(
            stall_lerp_color(Duration::from_secs(8), &theme),
            theme.error
        );
        assert_eq!(
            stall_lerp_color(Duration::from_secs(30), &theme),
            theme.error
        );
    }

    #[test]
    fn glyph_color_recovers_to_orange_when_deltas_resume() {
        // A fresh delta (last_delta_at == now) resets the stall → orange.
        let mut s = session_with(StreamingPhase::Drafting, 100);
        let now = Instant::now();
        // First: stalled 5s → reddened.
        s.last_delta_at = now - Duration::from_secs(5);
        let theme = Theme::hearth();
        let stalled = stall_glyph_color(&s, now, &theme);
        assert_ne!(stalled, theme.orange);
        // Then: delta resumes (last_delta_at advances to now) → back to orange.
        s.last_delta_at = now;
        assert_eq!(stall_glyph_color(&s, now, &theme), theme.orange);
    }

    #[test]
    fn stall_lerp_snaps_for_non_rgb_themes() {
        // 256-color / NO_COLOR have no continuous RGB space: stay on orange
        // until fully stalled, then snap to error (no fake gradient).
        for theme in [Theme::no_color(), Theme::hearth_256()] {
            assert_eq!(
                stall_lerp_color(Duration::from_secs(0), &theme),
                theme.orange
            );
            // Mid-stall snaps to the source color (no blend possible).
            assert_eq!(
                stall_lerp_color(Duration::from_secs(5), &theme),
                theme.orange
            );
            // Full stall flips to error so the signal is still visible.
            assert_eq!(
                stall_lerp_color(Duration::from_secs(10), &theme),
                theme.error
            );
        }
    }

    #[test]
    fn tip_shows_only_after_15_second_phase() {
        // Under threshold → no tip rendered.
        assert!(pick_tip(Duration::from_secs(0)).is_none());
        assert!(pick_tip(Duration::from_secs(14)).is_none());
        // At/over threshold → a tip is shown.
        assert!(pick_tip(Duration::from_secs(15)).is_some());
        assert!(pick_tip(Duration::from_secs(60)).is_some());
        // First tip lasts the full TIP_ROTATION_SECS window.
        let first = pick_tip(Duration::from_secs(15)).unwrap();
        let same = pick_tip(Duration::from_secs(44)).unwrap();
        assert_eq!(first, same);
        // Crossing the next 30s boundary advances the rotation.
        let next = pick_tip(Duration::from_secs(46)).unwrap();
        assert_ne!(first, next);
    }

    #[test]
    fn phase_transitions_on_protocol_events() {
        // Apply a synthesized event stream and assert phase per stage.
        // This is the bridge-side contract for D3: every transition tested.
        use crate::tui::app::App;
        use crate::tui::protocol_bridge::apply_event;
        use wcore_protocol::events::{
            FinishReason, OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus,
        };

        let mut app = App::new();

        // StreamStart → Thinking.
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        assert_eq!(app.session.phase, StreamingPhase::Thinking);
        assert!(app.session.streaming_active);

        // TextDelta → Drafting + delta watchdog refreshed.
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: "Hi".into(),
                msg_id: "m1".into(),
            },
        );
        assert_eq!(app.session.phase, StreamingPhase::Drafting);

        // ToolRequest → CallingTool(name).
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool: ToolInfo {
                    name: "web_search".into(),
                    category: wcore_protocol::events::ToolCategory::Info,
                    args: serde_json::json!({}),
                    description: String::new(),
                },
            },
        );
        assert_eq!(
            app.session.phase,
            StreamingPhase::CallingTool("web_search".into())
        );

        // ToolRunning → RunningTool(name).
        apply_event(
            &mut app,
            ProtocolEvent::ToolRunning {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "web_search".into(),
            },
        );
        assert_eq!(
            app.session.phase,
            StreamingPhase::RunningTool("web_search".into())
        );

        // ToolResult does NOT touch phase directly — the next TextDelta
        // pulls it back into Drafting.
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "c1".into(),
                tool_name: "web_search".into(),
                status: ToolStatus::Success,
                output: "ok".into(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::TextDelta {
                text: " more".into(),
                msg_id: "m1".into(),
            },
        );
        assert_eq!(app.session.phase, StreamingPhase::Drafting);

        // StreamEnd → Idle, with Usage tokens rolled up.
        apply_event(
            &mut app,
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: FinishReason::Stop,
                usage: Some(wcore_protocol::events::Usage {
                    input_tokens: 10,
                    output_tokens: 439,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    active_window_percent: None,
                }),
                agent_run_id: None,
            },
        );
        assert_eq!(app.session.phase, StreamingPhase::Idle);
        assert!(!app.session.streaming_active);
        assert_eq!(app.session.tokens_out, 439);
    }

    #[test]
    fn wrapping_up_transitions_after_15s_silent_drafting() {
        // Manually pin last_delta_at into the past and call the tick.
        let mut s = SessionView {
            streaming_active: true,
            phase: StreamingPhase::Drafting,
            ..Default::default()
        };
        let now = Instant::now();
        // 14s silence: still Drafting.
        s.last_delta_at = now - Duration::from_secs(14);
        s.tick_streaming_phase(now);
        assert_eq!(s.phase, StreamingPhase::Drafting);
        // 16s silence: now WrappingUp.
        s.last_delta_at = now - Duration::from_secs(16);
        s.tick_streaming_phase(now);
        assert_eq!(s.phase, StreamingPhase::WrappingUp);
    }

    #[test]
    fn render_includes_verb_elapsed_tokens_and_phase() {
        let mut s = session_with(StreamingPhase::Drafting, 439);
        // Pin the clocks 83s into the turn so we get a stable "1m 23s".
        let now = Instant::now();
        s.turn_started_at = now - Duration::from_secs(83);
        s.phase_started_at = now - Duration::from_secs(2);
        s.last_delta_at = now;
        let theme = Theme::hearth();
        let line = render_streaming_status_at(&s, 0, now, &theme);
        let rendered: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(rendered.contains("1m 23s"), "rendered: {rendered}");
        assert!(rendered.contains("↑ 439 tokens"), "rendered: {rendered}");
        assert!(rendered.contains("drafting reply"), "rendered: {rendered}");
        // No tip because the PHASE has only held for 2s (turn elapsed is
        // irrelevant for tip gating).
        assert!(!rendered.contains("tip"), "no tip expected: {rendered}");
        // First symbol frame.
        assert!(rendered.starts_with("✻"), "rendered: {rendered}");
    }

    #[test]
    fn symbol_animates_with_frame_tick() {
        let s = session_with(StreamingPhase::Drafting, 0);
        let theme = Theme::hearth();
        let now = Instant::now();
        let frame_a = render_streaming_status_at(&s, 0, now, &theme);
        let frame_b = render_streaming_status_at(&s, SYMBOL_TICKS_PER_FRAME, now, &theme);
        let a = frame_a.spans[0].content.as_ref().to_string();
        let b = frame_b.spans[0].content.as_ref().to_string();
        // Different first span → symbol advanced.
        assert_ne!(a, b, "symbol should rotate with frame_tick");
    }

    // ── v0.9.1.2 F14 — awaiting-approval phase replaces verb rotation ──

    #[test]
    fn streaming_status_shows_awaiting_not_brewing_v0912() {
        // When phase is AwaitingApproval, the widget MUST NOT render any
        // of the verb-rotation strings (Brewing / Leavening / Steeping
        // etc.) and MUST NOT render the "calling X" / "running X"
        // phase labels — those imply work is happening, when actually
        // the engine is parked waiting for the user's `y`.
        let mut s = session_with(
            StreamingPhase::AwaitingApproval {
                tool: "Write".into(),
                pending_count: 1,
            },
            0,
        );
        let now = Instant::now();
        s.turn_started_at = now;
        s.phase_started_at = now;
        s.last_delta_at = now;
        let theme = Theme::hearth();
        let line = render_streaming_status_at(&s, 0, now, &theme);
        let rendered: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        for verb in [
            "Brewing",
            "Leavening",
            "Steeping",
            "Pondering",
            "Spinning",
            "Considering",
            "Drafting",
            "Threading",
            "Surveying",
            "Mulling",
        ] {
            assert!(
                !rendered.contains(verb),
                "verb {verb:?} must not appear during awaiting-approval; got: {rendered}"
            );
        }
        for label in ["calling ", "running ", "wrapping up", "thinking"] {
            assert!(
                !rendered.contains(label),
                "phase label {label:?} must not appear during awaiting-approval; got: {rendered}"
            );
        }
        assert!(
            rendered.contains("Awaiting your approval"),
            "awaiting label missing: {rendered}"
        );
        assert!(rendered.contains("Write"), "tool name missing: {rendered}");
    }

    #[test]
    fn awaiting_approval_renders_more_pending_when_count_gt_1_v0912() {
        // `pending_count > 1` appends `(+N-1 more pending)` so a batch
        // is visible without the user having to count cards.
        let s = session_with(
            StreamingPhase::AwaitingApproval {
                tool: "Edit".into(),
                pending_count: 3,
            },
            0,
        );
        let theme = Theme::hearth();
        let now = Instant::now();
        let line = render_streaming_status_at(&s, 0, now, &theme);
        let rendered: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(
            rendered.contains("(+2 more pending)"),
            "batch tail missing: {rendered}"
        );
    }
}
