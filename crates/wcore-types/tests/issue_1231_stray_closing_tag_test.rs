//! FerroxLabs/wayland#1231 c5 — the STRAY CLOSING TAG shape, measured.
//!
//! The reporter's second symptom is *"sometimes just `</thought>` repeatedly,
//! the most so far has been 5 times along one row"*. That is an unmatched
//! CLOSING tag with no opener, which is a different input class from the
//! inline `<think>…</think>` block the #908 fix reasoned about. c5 asks
//! whether the filter is correct to consume it, and says that IF a bare
//! `</thought>` causes surviving answer text to be discarded, that is a filter
//! defect distinct from c2 with its own fix.
//!
//! MEASURED, and the antecedent is FALSE: an unmatched closing tag consumes
//! exactly itself and nothing around it. Answer text before it, after it, on
//! both sides of it, and split across delta boundaries all survive byte for
//! byte. There is no separate filter defect to fix, and this file is the
//! measurement standing as a guard so a later filter change cannot quietly
//! introduce one.
//!
//! What the measurement DOES explain is the reporter's symptom. Five bare
//! closing tags filter to the empty string — so a turn whose whole content is
//! those five tags is an empty turn, which is c2's shape, not a filter defect.
//! The two are the same bug seen through different provider output.

use wcore_types::reasoning_filter::ReasoningFilter;

/// Feed `chunks` through one filter exactly as the engine does — `process`
/// per delta, then `finish` at end of stream.
fn filtered(chunks: &[&str]) -> String {
    let mut filter = ReasoningFilter::new();
    let mut out = String::new();
    for chunk in chunks {
        out.push_str(&filter.process(chunk));
    }
    out.push_str(&filter.finish());
    out
}

#[test]
fn a_stray_closing_tag_consumes_itself_and_nothing_around_it() {
    // The reporter's exact shape: nothing but closing tags.
    assert_eq!(filtered(&["</thought>"]), "");
    assert_eq!(
        filtered(&["</thought></thought></thought></thought></thought>"]),
        "",
        "five bare closes in a row is the reporter's own description"
    );

    // The property c5 exists to decide: does answer text around one survive?
    assert_eq!(
        filtered(&["Paris is the capital.</thought>"]),
        "Paris is the capital.",
        "answer text BEFORE a stray close must survive it"
    );
    assert_eq!(
        filtered(&["</thought>Paris is the capital."]),
        "Paris is the capital.",
        "answer text AFTER a stray close must survive it"
    );
    assert_eq!(
        filtered(&["Paris</thought> is the capital."]),
        "Paris is the capital.",
        "a stray close BETWEEN two runs of answer text must not eat either"
    );
    // Split across a delta boundary, because that is how it actually arrives:
    // the tag is buffered as an ambiguous prefix across two chunks, which is
    // the path most likely to over-consume.
    assert_eq!(
        filtered(&["Paris</th", "ought> is the capital."]),
        "Paris is the capital.",
        "a stray close split across two deltas must behave identically"
    );
}

/// CONTROLS. Without these, a filter that had simply stopped stripping
/// anything would satisfy every assertion above.
#[test]
fn control_a_matched_block_is_still_stripped_and_untagged_text_is_untouched() {
    assert_eq!(
        filtered(&["<thought>reasoning</thought>Paris."]),
        "Paris.",
        "control: a MATCHED block is still stripped, so the filter is running"
    );
    assert_eq!(
        filtered(&["Paris is the capital."]),
        "Paris is the capital.",
        "control: text with no tags at all is passed through unchanged"
    );
    assert_ne!(
        filtered(&["<thought>reasoning</thought>Paris."]),
        filtered(&["Paris is the capital."]),
        "control: the two control arms differ, so the assertions can fail"
    );
}
