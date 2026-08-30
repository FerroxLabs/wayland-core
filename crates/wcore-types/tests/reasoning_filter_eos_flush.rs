//! wayland#1222 — `ReasoningFilter::finish`, the end-of-stream drain.
//!
//! `process` withholds two things: an undecided `<`-prefix, and everything
//! after an opening reasoning tag until that tag closes. Before `finish` both
//! were discarded when the stream ended. These tests pin the DECISION taken
//! for each (recorded in the module docs of `reasoning_filter.rs`), and pin
//! the retraction that stops recovered prose from ALSO being reported as
//! reasoning.

use wcore_types::reasoning_filter::ReasoningFilter;

/// Feed the whole input as one chunk and return `(visible, captured)` the way
/// a consumer that drains at end of stream sees them.
fn run(input: &str) -> (String, String) {
    let mut f = ReasoningFilter::new();
    let mut visible = f.process(input);
    visible.push_str(&f.finish());
    (visible, f.take_captured())
}

// ── the pending `<`-prefix is text after all ────────────────────────────────

#[test]
fn a_trailing_angle_bracket_is_flushed_as_text() {
    assert_eq!(
        run("the answer is 5 <"),
        ("the answer is 5 <".into(), String::new())
    );
}

#[test]
fn a_partial_tag_prefix_is_flushed_as_text() {
    assert_eq!(run("result: <th"), ("result: <th".into(), String::new()));
}

#[test]
fn a_partial_close_prefix_is_flushed_as_text() {
    assert_eq!(run("done </thi"), ("done </thi".into(), String::new()));
}

// ── an unclosed open is RECOVERED, not eaten ────────────────────────────────

#[test]
fn an_unclosed_open_is_recovered_verbatim_including_its_tag() {
    const INPUT: &str = "Use the <thinking> tag to wrap reasoning. Then answer.";
    assert_eq!(run(INPUT), (INPUT.into(), String::new()));
}

#[test]
fn an_unclosed_open_with_attributes_is_recovered_verbatim() {
    const INPUT: &str = "see <thinking depth=\"2\"> for the shape";
    assert_eq!(run(INPUT), (INPUT.into(), String::new()));
}

/// The retraction is the load-bearing half: without it the recovered prose is
/// emitted as text AND handed to the caller as a reasoning body, so a host
/// renders the same sentence twice.
#[test]
fn recovered_prose_is_retracted_from_the_capture_buffer() {
    let mut f = ReasoningFilter::new();
    let visible = f.process("a<thinking>b");
    assert_eq!(
        visible, "a",
        "precondition: `process` withholds the open block"
    );
    assert_eq!(
        f.finish(),
        "<thinking>b",
        "the drain must return the raw bytes from the opening tag onward"
    );
    assert_eq!(
        f.take_captured(),
        "",
        "the recovered prose must not ALSO be reported as reasoning"
    );
}

/// A CLOSED block before an unclosed one: the closed body stays captured, the
/// unclosed one comes back as text. Truncating the capture buffer to the wrong
/// offset would eat the earlier block.
#[test]
fn a_closed_block_survives_a_later_unclosed_one() {
    let mut f = ReasoningFilter::new();
    let mut visible = f.process("<think>kept</think>answer <think>runaway");
    visible.push_str(&f.finish());
    assert_eq!(visible, "answer <think>runaway");
    assert_eq!(f.take_captured(), "kept");
}

/// Nested opens inside the unclosed block are part of the raw recovery.
#[test]
fn nested_tags_inside_an_unclosed_block_are_recovered_verbatim() {
    const INPUT: &str = "x <think>outer <think>inner</think> still open";
    let (visible, captured) = run(INPUT);
    assert_eq!(visible, INPUT);
    assert_eq!(captured, "");
}

// ── the filter must not become a no-op ──────────────────────────────────────

/// #908 c1's behaviour, unchanged: a block that CLOSES is stripped and its body
/// is reported as reasoning. This is the control that stops "recover
/// everything" passing as a fix.
#[test]
fn control_a_closed_block_is_still_stripped_and_captured() {
    assert_eq!(run("<think>plan</think>42"), ("42".into(), "plan".into()),);
}

/// The ticket's own controls, at the filter boundary.
#[test]
fn control_the_narrow_cases_stay_byte_exact() {
    for input in ["if a < b then", "if a <b then c", "<div>hello</div>"] {
        assert_eq!(run(input), (input.into(), String::new()), "control {input}");
    }
}

#[test]
fn control_a_self_closing_tag_still_vanishes() {
    assert_eq!(run("a<think/>b"), ("ab".into(), String::new()));
}

#[test]
fn control_a_stray_close_is_still_dropped() {
    assert_eq!(
        run("The answer is 42.</thought>"),
        ("The answer is 42.".into(), String::new())
    );
}

// ── drain shape ─────────────────────────────────────────────────────────────

#[test]
fn finish_is_idempotent_and_empty_when_nothing_is_held_back() {
    let mut f = ReasoningFilter::new();
    assert_eq!(f.process("plain text"), "plain text");
    assert_eq!(f.finish(), "");
    assert_eq!(f.finish(), "");

    let mut g = ReasoningFilter::new();
    assert_eq!(g.process("tail <th"), "tail ");
    assert_eq!(g.finish(), "<th");
    assert_eq!(g.finish(), "", "a second drain must not repeat the buffer");
}

/// `reset` clears an in-flight recovery, so a cancelled stream cannot leak its
/// unclosed block into the next turn.
#[test]
fn reset_discards_a_pending_recovery() {
    let mut f = ReasoningFilter::new();
    let _ = f.process("a<think>runaway");
    f.reset();
    assert_eq!(f.finish(), "");
    assert_eq!(f.process("b"), "b");
}
