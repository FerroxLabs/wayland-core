//! Streaming-safe markdown split-point helper.
//!
//! When markdown is streamed in chunks, rendering the partial buffer
//! mid-fence or mid-link produces visual flicker (half-rendered code
//! blocks, broken links). [`last_safe_split_point`] walks the buffer as
//! a small state machine and returns the byte offset of the last
//! newline at which every markdown construct is closed.
//!
//! The caller (TUI protocol_bridge / workspace renderer) splits the
//! streaming buffer at that offset, renders the safe prefix, and keeps
//! the unsafe suffix in the streaming buffer until more bytes arrive.
//!
//! Unsafe positions tracked:
//! - Inside an unclosed ``` ``` ``` fence (backtick or tilde, 3+)
//! - Inside an unclosed single-backtick inline code span
//! - Inside an unclosed `[link text` (no `](` seen yet)
//! - Inside an unclosed `[link text](url` (no closing `)` yet)
//! - Inside an unclosed table row (line starts with `|`, no `\n` yet)

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkState {
    Closed,
    InText,
    InUrl,
}

/// Returns the byte offset of the last safe split point in `src`, or
/// `src.len()` if the whole string is safely renderable. Returns `0` if
/// every position is unsafe (e.g. the input is one open fence).
pub fn last_safe_split_point(src: &str) -> usize {
    if src.is_empty() {
        return 0;
    }

    let bytes = src.as_bytes();
    let n = bytes.len();

    let mut fence_open = false;
    // The opening fence character: `` ` `` or `~`. Closing fence must
    // match — `` ``` `` opened with backticks can only close with
    // backticks, not tildes.
    let mut fence_char: u8 = 0;
    let mut inline_code_open = false;
    let mut link_state = LinkState::Closed;
    let mut table_line_open = false;
    // True when we have only seen whitespace on the current line so far
    // (used to detect fences and table rows, which must start a line).
    let mut at_line_start = true;

    // Candidate safe split: the byte offset *just after* the last
    // newline where every state was Closed/Inactive. `src.len()` if the
    // very end is also safe.
    let mut last_safe: usize = 0;

    let mut i: usize = 0;
    while i < n {
        let c = bytes[i];

        // --- Inside an open fenced code block ---
        // Only thing that matters is a matching closing fence on its
        // own line.
        if fence_open {
            if c == b'\n' {
                at_line_start = true;
                i += 1;
                continue;
            }
            if at_line_start && (c == b' ' || c == b'\t') {
                i += 1;
                continue;
            }
            // Try to close the fence: 3+ of fence_char at line start
            // (allow trailing chars on the line; markdown allows a
            // language after opening fences but a closing fence is
            // typically bare — we accept 3+ matching chars followed by
            // anything until newline).
            if at_line_start && c == fence_char {
                let mut run = 0;
                let mut j = i;
                while j < n && bytes[j] == fence_char {
                    run += 1;
                    j += 1;
                }
                if run >= 3 {
                    fence_open = false;
                    fence_char = 0;
                    at_line_start = false;
                    i = j;
                    continue;
                }
            }
            at_line_start = false;
            i += 1;
            continue;
        }

        // --- Inside an open inline code span ---
        if inline_code_open {
            if c == b'`' {
                inline_code_open = false;
                i += 1;
                continue;
            }
            if c == b'\n' {
                // Inline code does not survive a blank line in CommonMark,
                // but for streaming safety we still mark the buffer
                // unsafe until we see the closing tick. Newlines inside
                // open inline code just advance.
                at_line_start = true;
                i += 1;
                continue;
            }
            at_line_start = false;
            i += 1;
            continue;
        }

        // --- Newline boundary: candidate split point ---
        if c == b'\n' {
            // Close current-line table state at newline.
            table_line_open = false;
            // A safe split is possible iff every multi-line state is
            // closed. (Inline link/code/fence states are all closed
            // here because the earlier branches handled them.)
            if !fence_open
                && !inline_code_open
                && link_state == LinkState::Closed
                && !table_line_open
            {
                last_safe = i + 1;
            }
            at_line_start = true;
            i += 1;
            continue;
        }

        // --- Whitespace at line start: keep at_line_start sticky ---
        if at_line_start && (c == b' ' || c == b'\t' || c == b'\r') {
            i += 1;
            continue;
        }

        // --- Fence open detection: 3+ backticks or tildes at line start ---
        if at_line_start && (c == b'`' || c == b'~') {
            let fc = c;
            let mut run = 0;
            let mut j = i;
            while j < n && bytes[j] == fc {
                run += 1;
                j += 1;
            }
            if run >= 3 {
                fence_open = true;
                fence_char = fc;
                at_line_start = false;
                i = j;
                continue;
            }
            // Fewer than 3 of this char at line start: fall through to
            // inline-code / regular char handling. Backtick triggers
            // inline code; tilde is just a literal.
            if fc == b'`' {
                inline_code_open = true;
                at_line_start = false;
                i += 1;
                continue;
            }
            // Tilde literal.
            at_line_start = false;
            i += 1;
            continue;
        }

        // --- Table row detection: line begins with `|` ---
        if at_line_start && c == b'|' {
            table_line_open = true;
            at_line_start = false;
            i += 1;
            continue;
        }

        // --- Inline code open (mid-line single backtick) ---
        if c == b'`' {
            // Distinguish single from triple+ even mid-line: if we see
            // 3+ backticks, treat as an inline-code toggle of a long
            // run rather than a fence (fences must start a line). We
            // model any run of backticks as a code span delimiter that
            // closes on a matching run. Simple heuristic: a single ` is
            // the common streaming case; just toggle.
            inline_code_open = true;
            at_line_start = false;
            i += 1;
            continue;
        }

        // --- Link state machine ---
        match link_state {
            LinkState::Closed => {
                if c == b'[' {
                    link_state = LinkState::InText;
                    at_line_start = false;
                    i += 1;
                    continue;
                }
            }
            LinkState::InText => {
                if c == b']' {
                    // Need an immediate `(` to enter InUrl, else link
                    // is just `[...]` (reference-style, safe).
                    if i + 1 < n && bytes[i + 1] == b'(' {
                        link_state = LinkState::InUrl;
                        at_line_start = false;
                        i += 2;
                        continue;
                    }
                    link_state = LinkState::Closed;
                    at_line_start = false;
                    i += 1;
                    continue;
                }
                if c == b'\n' {
                    // Multi-line link text: clear table flag, keep
                    // link_state open, fall through to general newline
                    // logic which marks this offset UNSAFE because
                    // link_state != Closed.
                    table_line_open = false;
                    at_line_start = true;
                    i += 1;
                    continue;
                }
            }
            LinkState::InUrl => {
                if c == b')' {
                    link_state = LinkState::Closed;
                    at_line_start = false;
                    i += 1;
                    continue;
                }
                if c == b'\n' {
                    table_line_open = false;
                    at_line_start = true;
                    i += 1;
                    continue;
                }
            }
        }

        at_line_start = false;
        i += 1;
    }

    // After the loop, if every state is closed and we ended on a
    // newline boundary we already recorded last_safe = n. If the buffer
    // ended mid-line but with all states closed, the *whole* buffer is
    // safe to render — return n.
    if !fence_open && !inline_code_open && link_state == LinkState::Closed && !table_line_open {
        return n;
    }

    last_safe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_string_safe_when_plain_text() {
        let src = "Hello world\n";
        assert_eq!(last_safe_split_point(src), src.len());
    }

    #[test]
    fn unclosed_code_fence_blocks_everything_after_open() {
        let src = "Hi\n```rust\nfn foo";
        // Safe split is right after `Hi\n`, i.e. byte offset 3.
        assert_eq!(last_safe_split_point(src), 3);
    }

    #[test]
    fn closed_code_fence_safe() {
        let src = "```rust\nfn foo() {}\n```\n";
        assert_eq!(last_safe_split_point(src), src.len());
    }

    #[test]
    fn unclosed_inline_code() {
        // "Hello `foo" — opens inline code, never closes.
        let src = "Hello `foo";
        // No newline reached with all states closed → 0.
        assert_eq!(last_safe_split_point(src), 0);
    }

    #[test]
    fn unclosed_link_text() {
        let src = "See [the docs";
        assert_eq!(last_safe_split_point(src), 0);
    }

    #[test]
    fn unclosed_link_url() {
        let src = "See [docs](https://";
        assert_eq!(last_safe_split_point(src), 0);
    }

    #[test]
    fn closed_link_safe() {
        let src = "See [docs](https://example.com)";
        assert_eq!(last_safe_split_point(src), src.len());
    }

    #[test]
    fn unclosed_table_row() {
        let src = "| col1 | col2";
        // No newline ever reached → 0.
        assert_eq!(last_safe_split_point(src), 0);
    }

    #[test]
    fn mixed_content_only_unclosed_stops() {
        let src = "First paragraph.\n\nSecond paragraph.\n\n```rust\nlet x";
        // Last safe newline is the second blank-line newline right
        // after "Second paragraph.\n" — i.e. offset of the byte
        // immediately after the second `\n\n`.
        let expected = "First paragraph.\n\nSecond paragraph.\n\n".len();
        assert_eq!(last_safe_split_point(src), expected);
    }

    #[test]
    fn empty_string_returns_zero() {
        assert_eq!(last_safe_split_point(""), 0);
    }

    #[test]
    fn single_newline_safe() {
        assert_eq!(last_safe_split_point("\n"), 1);
    }

    // --- Additional edge cases ---

    #[test]
    fn tilde_fence_same_as_backtick() {
        let src = "Intro\n~~~\ncode\n";
        // Unclosed tilde fence — safe split is right after "Intro\n".
        assert_eq!(last_safe_split_point(src), 6);
    }

    #[test]
    fn closed_tilde_fence_safe() {
        let src = "~~~\ncode\n~~~\n";
        assert_eq!(last_safe_split_point(src), src.len());
    }

    #[test]
    fn fence_with_mismatched_closer_stays_open() {
        // Open with backticks; tildes do NOT close it.
        let src = "```\ncode\n~~~\n";
        // Whole thing is inside the open fence: no safe split.
        assert_eq!(last_safe_split_point(src), 0);
    }

    #[test]
    fn crlf_line_endings() {
        // CRLF: \r is treated as a regular char before \n. The newline
        // is what marks the candidate split, so safe offset is just
        // after \n.
        let src = "Hi\r\nWorld\r\n";
        assert_eq!(last_safe_split_point(src), src.len());
    }

    #[test]
    fn reference_style_link_safe() {
        // `[text]` with no `(` after `]` is reference-style — safe.
        let src = "See [the docs]\n";
        assert_eq!(last_safe_split_point(src), src.len());
    }

    #[test]
    fn closed_table_row() {
        // Table row that ends with a newline is safe.
        let src = "| a | b |\n| c | d |\n";
        assert_eq!(last_safe_split_point(src), src.len());
    }

    #[test]
    fn unclosed_inline_then_newline_in_middle() {
        // Plain newline then an unclosed inline code at the tail.
        let src = "intro\n`unclosed";
        assert_eq!(last_safe_split_point(src), 6);
    }
}
