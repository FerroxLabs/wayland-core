//! Outbound message chunking.
//!
//! Every chat platform caps a single message's length (Discord 2000,
//! Telegram/WhatsApp 4096, Slack 4000, MS Teams 20480, Matrix 16384, SMS 1600
//! — `docs/delivery-semantics.md` §4.2 has every cap and its source). An agent
//! reply that exceeds the cap was previously **rejected by the platform and
//! silently dropped** (HIGH-6). [`chunk_message`] splits an over-long body
//! into platform-sized pieces the sender delivers in order.
//!
//! Length is measured in **Unicode scalar values** (`char`s). This is exact
//! for the ASCII/BMP text that dominates chat and never splits a codepoint.
//! Platforms that count UTF-16 code units (Telegram, Discord) could see a
//! message composed almost entirely of astral-plane characters (e.g. many
//! emoji) measure higher on their side than our scalar count; the
//! per-connector caps are set at the documented limits, so keep that astral
//! caveat in mind if a connector ever reports truncation. Splitting always
//! happens on a `char` boundary regardless.

/// Split `text` into chunks of at most `max_len` `char`s each, preferring to
/// break at the last newline (then the last ASCII space) inside the window
/// so words/lines stay intact; falls back to a hard `char`-boundary split
/// for a single unbroken run longer than `max_len`.
///
/// Returns the text unchanged (as a single chunk) when it already fits or
/// when `max_len == 0` (cap unknown/disabled). Never returns an empty
/// `Vec` for non-empty input, and never an empty chunk. Empty input returns
/// a single empty chunk so callers preserve their existing "send this body"
/// semantics rather than silently sending nothing.
pub fn chunk_message(text: &str, max_len: usize) -> Vec<String> {
    let total = text.chars().count();
    if max_len == 0 || total <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    // Work over char indices so every split lands on a codepoint boundary.
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0usize;
    while start < chars.len() {
        let hard_end = (start + max_len).min(chars.len());
        // If this is the final piece, take it all.
        if hard_end == chars.len() {
            chunks.push(chars[start..hard_end].iter().collect());
            break;
        }
        // Otherwise prefer a soft break (newline, then space) within
        // [start, hard_end). Search from the window end backwards.
        let window = &chars[start..hard_end];
        let split_at = window
            .iter()
            .rposition(|&c| c == '\n')
            .or_else(|| window.iter().rposition(|&c| c == ' '))
            // Keep at least one char of progress: a break at index 0 would
            // loop forever, so reject it and hard-split instead.
            .filter(|&idx| idx > 0)
            .map(|idx| start + idx + 1) // include the break char in this chunk
            .unwrap_or(hard_end);

        chunks.push(chars[start..split_at].iter().collect::<String>());
        start = split_at;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_single_chunk() {
        assert_eq!(chunk_message("hello", 100), vec!["hello"]);
    }

    #[test]
    fn empty_text_is_single_empty_chunk() {
        assert_eq!(chunk_message("", 100), vec![""]);
    }

    #[test]
    fn zero_max_disables_chunking() {
        let long = "x".repeat(10_000);
        assert_eq!(chunk_message(&long, 0), vec![long]);
    }

    #[test]
    fn hard_split_for_unbroken_run() {
        let s = "abcdefghij"; // 10 chars, no break points
        let out = chunk_message(s, 4);
        assert_eq!(out, vec!["abcd", "efgh", "ij"]);
        // Every chunk is within the cap.
        assert!(out.iter().all(|c| c.chars().count() <= 4));
        // Lossless reassembly.
        assert_eq!(out.concat(), s);
    }

    #[test]
    fn prefers_newline_break() {
        let s = "line one\nline two\nline three";
        let out = chunk_message(s, 12);
        // First chunk ends at the first newline (kept), not mid-word.
        assert_eq!(out[0], "line one\n");
        assert_eq!(out.concat(), s);
        assert!(out.iter().all(|c| c.chars().count() <= 12));
    }

    #[test]
    fn prefers_space_break_when_no_newline() {
        let s = "the quick brown fox";
        let out = chunk_message(s, 10);
        assert_eq!(out[0], "the quick "); // breaks after the space within window
        assert_eq!(out.concat(), s);
        assert!(out.iter().all(|c| c.chars().count() <= 10));
    }

    #[test]
    fn never_splits_a_codepoint() {
        // 8 multi-byte chars; cap 3 → chunks of 3,3,2 chars, all valid UTF-8.
        let s = "αβγδεζηθ";
        let out = chunk_message(s, 3);
        assert_eq!(
            out.iter().map(|c| c.chars().count()).collect::<Vec<_>>(),
            vec![3, 3, 2]
        );
        assert_eq!(out.concat(), s);
    }

    #[test]
    fn leading_break_char_does_not_loop() {
        // A space at index 0 of the window must NOT be chosen (would not make
        // progress); the run hard-splits instead.
        let s = " aaaaaaaa"; // leading space then 8 a's
        let out = chunk_message(s, 4);
        assert_eq!(out.concat(), s);
        assert!(out.iter().all(|c| !c.is_empty() && c.chars().count() <= 4));
    }
}
