//! Integration coverage for the streaming reasoning filter.
//!
//! The filter shipped in v0.13.7 with unit tests only; nothing exercised it
//! from outside the crate, so the stray-close leak below survived every green
//! suite. These tests drive the public API the way a sink does.

use wcore_types::reasoning_filter::ReasoningFilter;

/// Feed `chunks` through one filter and concatenate the visible output.
fn run(chunks: &[&str]) -> String {
    let mut filter = ReasoningFilter::new();
    let mut out = String::new();
    for chunk in chunks {
        out.push_str(&filter.process(chunk));
    }
    out
}

/// Feed every char of `s` separately — the token-stream adversary.
fn run_char_by_char(s: &str) -> String {
    let mut filter = ReasoningFilter::new();
    let mut out = String::new();
    for ch in s.chars() {
        out.push_str(&filter.process(&ch.to_string()));
    }
    out
}

#[test]
fn stray_close_think_never_reaches_visible_output() {
    assert_eq!(run(&["plain </think> text"]), "plain  text");
}

#[test]
fn stray_close_thought_never_reaches_visible_output() {
    // The reported shape (#908): the model closes a reasoning block whose
    // opener the provider already consumed, so the sink sees only `</thought>`.
    assert_eq!(run(&["The answer is 42.</thought>"]), "The answer is 42.");
}

#[test]
fn stray_close_split_across_chunk_boundary_is_dropped() {
    assert_eq!(run(&["answer</thou", "ght>tail"]), "answertail");
}

#[test]
fn stray_close_char_by_char_is_dropped() {
    assert_eq!(run_char_by_char("a</reasoning>b"), "ab");
}

#[test]
fn stray_close_after_a_completed_block_is_dropped() {
    assert_eq!(run(&["<think>hidden</think>visible</thought>"]), "visible");
}

#[test]
fn unrecognised_close_tag_still_passes_through_as_plain_text() {
    // This is a reasoning filter, not an HTML sanitiser: `</b>` is user text.
    assert_eq!(run(&["a</b>c"]), "a</b>c");
    assert_eq!(run(&["x</thigh>y"]), "x</thigh>y");
}

#[test]
fn stray_close_does_not_capture_reasoning_body() {
    // A close with no opener has no body — it must not fabricate one.
    let mut filter = ReasoningFilter::new();
    let visible = filter.process("hi</thought>there");
    assert_eq!(visible, "hithere");
    assert_eq!(filter.take_captured(), "");
}
