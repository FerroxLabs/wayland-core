//! The whitespace-split redaction pass must redact the SECRET, not the page.
//!
//! On the result-truncation path the scrubber replaced an entire multi-kilobyte
//! whitespace-spanning run with one 34-char marker, destroying tens of KB of
//! legitimate file content and its line numbers with no indication of scale.
//!
//! Three parts interacted. (1) The wrapped-base64 candidate regex matches any
//! run of alphanumerics + ASCII whitespace + `+/_=`, unbounded — ordinary
//! punctuation-free prose IS such a run, so a 2001-line numbered Read result is
//! ONE candidate. (2) The candidate is whitespace-normalized into a single line
//! before every pattern is re-run against it. (3) `SECRET_ASSIGNMENT`'s key
//! prefix `(?:[A-Z][A-Z0-9]*_)*` is unbounded in both repetition count and
//! per-segment length, and once the newlines are gone `(?m)^` has exactly one
//! anchor — offset 0 — so the prefix walks the whole blob until it reaches a
//! literal `TOKEN`. (4) The redaction unit was then the CANDIDATE, not the
//! match.
//!
//! Truncation CREATES the match: at full length the run begins on a line
//! number, i.e. a DIGIT, which the key prefix cannot start on. `truncate_result`
//! cuts mid-line, so the tail run begins on a LETTER. Every fixture here
//! therefore begins on a letter, exactly as the tail slice does.

use std::borrow::Cow;
use wcore_safety::PIIScrubber;

/// 1200 numbered lines of punctuation-free prose, sliced to begin on a letter.
/// Mirrors the shipped Read result shape (`read.rs` emits `{:>6}\t{line}`).
fn numbered_prose() -> String {
    let mut body = String::new();
    for i in 1..=1200 {
        body.push_str(&format!(
            "{i:>6}\tfiller line {i} for the audit corpus of punctuation free prose\n"
        ));
    }
    body.push_str("  1201\tTAIL_CANARY_TOKEN=PLINTHWORM\n");
    let cut = body.find("filler").expect("fixture");
    body[cut..].to_string()
}

fn assert_prose_survived(out: &str, payload: &str, which: &str) {
    assert!(
        out.contains("filler line 900 for the audit corpus"),
        "{which}: mid-file line destroyed; got {} bytes: {}",
        out.len(),
        out.chars().take(200).collect::<String>()
    );
    assert!(
        out.contains("\n  1100\t"),
        "{which}: line numbering destroyed"
    );
    assert!(
        out.lines().count() >= 1_100,
        "{which}: line structure collapsed to {} lines",
        out.lines().count()
    );
    assert!(
        out.len() * 10 >= payload.len() * 9,
        "{which}: scrubber removed {} of {} bytes",
        payload.len().saturating_sub(out.len()),
        payload.len()
    );
}

/// The FAST path (`wrapped_record`), taken whenever the whole output is
/// punctuation-free — which a numbered plain Read result is.
#[test]
fn punctuation_free_prose_is_not_swallowed_by_the_fast_path() {
    let payload = numbered_prose();
    let out = PIIScrubber.scrub(&payload);
    assert_prose_survived(&out, &payload, "fast path");
}

/// The WRAPPED LOOP. One `.` is enough to make `wrapped_record` false, which is
/// the path the defect was actually reported on and which no test covered.
#[test]
fn punctuation_free_prose_is_not_swallowed_by_the_wrapped_loop() {
    let payload = format!(". {}", numbered_prose());
    let out = PIIScrubber.scrub(&payload);
    assert_prose_survived(&out, &payload, "wrapped loop");
}

/// CRLF sibling — `\r` is inside the candidate continuation class AND is
/// stripped by `is_ascii_whitespace`, so the offset map must survive it.
#[test]
fn crlf_prose_is_not_swallowed_either() {
    let payload = numbered_prose().replace('\n', "\r\n");
    let out = PIIScrubber.scrub(&payload);
    assert!(
        out.contains("filler line 900 for the audit corpus"),
        "CRLF: mid-file line destroyed: {}",
        out.chars().take(200).collect::<String>()
    );
    assert!(
        out.len() * 10 >= payload.len() * 9,
        "CRLF: scrubber removed {} of {} bytes",
        payload.len().saturating_sub(out.len()),
        payload.len()
    );
}

/// The amplifier itself, on a RAW single line with no whitespace at all — so
/// this is the direct pass, not the normalized re-scan. An unbounded key prefix
/// lets one `TOKEN=` at the end swallow every byte before it.
#[test]
fn secret_assignment_cannot_walk_back_across_a_whole_blob() {
    let blob = format!(
        "{}TAIL_CANARY_TOKEN=PLINTHWORM",
        "fortheauditcorpus".repeat(2_000)
    );
    let out = PIIScrubber.scrub(&blob);
    assert!(
        out.contains("fortheauditcorpus"),
        "a 34 KB blob was replaced wholesale by a {}-byte marker: {out}",
        out.len()
    );
}

/// The anti-vacuity twin: punishing over-redaction must not license
/// under-redaction. A "just delete the whitespace-split pass" fix fails the
/// first two assertions.
///
/// The trailing prose is separated from the secret by a non-alphanumeric
/// character in the ORIGINAL, because every open-ended pattern (GITHUB_PAT is
/// `ghp_[A-Za-z0-9]{20,}`) is greedy and, with whitespace deleted, would
/// otherwise run to the end of the candidate.
#[test]
fn newline_split_secret_inside_prose_is_redacted_without_eating_the_prose() {
    let before = "MARLINSPIKE7741 ".repeat(8);
    let after = ". QUARTZBADGER8820 is unrelated trailing prose";
    let payload = format!("{before}\ngh\np_abcdefghijklmnopqrstuvwxyz0123\n{after}");
    let out = PIIScrubber.scrub(&payload);

    assert!(
        out.contains("[REDACTED:"),
        "the split secret escaped entirely: {out}"
    );
    assert!(
        !out.contains("gh\np_"),
        "raw credential material emitted: {out}"
    );
    assert!(
        out.contains("MARLINSPIKE7741"),
        "prose before the secret destroyed: {out}"
    );
    assert!(
        out.contains("QUARTZBADGER8820"),
        "prose after the secret destroyed: {out}"
    );
}

/// Two distinct secrets inside ONE candidate run must each get their own
/// marker, and the text between them must survive. This is the case that a
/// botched span merge (unsorted or unmerged spans) turns into either a panic
/// or a leak.
#[test]
fn two_secrets_in_one_run_each_get_their_own_marker() {
    let payload = concat!(
        "ATLASFERRET3310 ghp_aaaaaaaaaaaaaaaaaaaaaa ",
        "MIDPROSE_SURVIVOR9915 ghp_bbbbbbbbbbbbbbbbbbbbbb ",
        "TRAILPROSE_KEEPER2277."
    );
    let out = PIIScrubber.scrub(payload);
    assert_eq!(
        out.matches("[REDACTED:GITHUB_PAT]").count(),
        2,
        "each secret needs its own marker: {out}"
    );
    assert!(
        out.contains("ATLASFERRET3310"),
        "leading prose eaten: {out}"
    );
    assert!(
        out.contains("MIDPROSE_SURVIVOR9915"),
        "prose BETWEEN the two secrets eaten: {out}"
    );
    assert!(
        out.contains("TRAILPROSE_KEEPER2277"),
        "trailing prose eaten: {out}"
    );
    assert!(!out.contains("ghp_aaaa"), "raw credential emitted: {out}");
    assert!(!out.contains("ghp_bbbb"), "raw credential emitted: {out}");
}

/// The RAW direct pass must not change. Today `scrub_direct` is a cascade of
/// sequential `replace_all`s in PATTERNS order, and the `[REDACTED:` guard
/// exists so `SECRET_ASSIGNMENT` preserves a line an earlier pattern already
/// marked. Reimplementing it as one merged span splice would make that guard
/// vacuous and turn this input into `[REDACTED:SECRET_ASSIGNMENT]`.
#[test]
fn the_direct_pass_still_cascades_rather_than_taking_the_leftmost_label() {
    let out = PIIScrubber.scrub("API_KEY=ghp_aaaaaaaaaaaaaaaaaaaaaa\n");
    assert!(
        out.contains("API_KEY=[REDACTED:GITHUB_PAT]"),
        "the direct pass changed shape: {out}"
    );
}

/// A genuinely newline-split assignment is still caught. This is the coverage
/// the rejected "skip line-anchored patterns in the normalized re-scan" change
/// would have silently dropped: the raw pass does NOT match it.
#[test]
fn a_newline_split_assignment_is_still_redacted() {
    let out = PIIScrubber.scrub("API_\nKEY=supersecretvalue123456");
    assert!(
        matches!(out, Cow::Owned(_)) && out.contains("[REDACTED:"),
        "a newline-split assignment must still be redacted: {out}"
    );
    assert!(
        !out.contains("supersecretvalue123456"),
        "the value leaked: {out}"
    );
}

/// The bounded key prefix is a strict NARROWING and must be stated, not
/// assumed: eight underscore-separated segments still match.
#[test]
fn a_realistically_long_key_still_matches() {
    let out = PIIScrubber.scrub("A_B_C_D_E_F_G_TOKEN=x\n");
    assert!(
        out.contains("[REDACTED:SECRET_ASSIGNMENT]"),
        "a real multi-segment env key must still redact: {out}"
    );
}

/// Multibyte document. The candidate character classes are ASCII-only, so a
/// CJK codepoint can never sit INSIDE a match — but it can sit either side of
/// one, and the splice arithmetic is byte-indexed. `map[end - 1].1` carries the
/// original char's UTF-8 length precisely so a naive `+ 1` cannot slice a
/// codepoint in half.
#[test]
fn multibyte_text_around_a_split_secret_is_preserved_byte_for_byte() {
    let payload = "前置きの文章です。\nghp_ccccccccccccccccccccc\n後続の文章です。";
    let out = PIIScrubber.scrub(payload);

    assert!(
        out.contains("[REDACTED:GITHUB_PAT]"),
        "the secret must still be redacted: {out}"
    );
    assert!(!out.contains("ghp_cccc"), "raw credential emitted: {out}");
    assert!(
        out.contains("前置きの文章です。"),
        "leading CJK destroyed: {out}"
    );
    assert!(
        out.contains("後続の文章です。"),
        "trailing CJK destroyed: {out}"
    );
}
