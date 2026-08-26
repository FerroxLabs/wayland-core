//! Streaming reasoning-tag filter.
//!
//! Lives in `wcore-types` — the lowest crate in the graph — because two
//! independent consumers need it and neither may depend on the other:
//!
//! * `wcore-cli`'s TUI, which strips reasoning out of the visible streaming
//!   buffer and renders the captured body as a collapsed `Thought:` block.
//! * `wcore-agent`'s `ProtocolSink`, which strips it out of `text_delta` and
//!   re-emits it as a typed `thinking` event so a JSON-stream host (the
//!   Wayland desktop app) can render it the same way (#1129).
//!
//! It was previously private to `wcore-cli/src/tui/render/`, which made the
//! protocol path architecturally unable to reach it: `wcore-agent` does not
//! (and must not) depend on `wcore-cli`. It is a dependency-free `&str` state
//! machine, so it relocates cleanly; it sits beside `utf8_stream`, the other
//! chunk-boundary state machine over provider output.
//!
//! Open-weights LLMs (DeepSeek-R1, Qwen-QwQ, etc.) emit private reasoning
//! inline in their text stream wrapped in `<think>...</think>`,
//! `<reasoning>...</reasoning>`, or `<thinking>...</thinking>` tags. The
//! engine does not strip these for raw providers (see
//! `.planning/recon/2026-05-27-reasoning-strip-audit.md`), so the TUI does
//! it host-side before the text reaches the visible streaming buffer.
//!
//! The filter is a small state machine designed to handle tags that split
//! across token-chunk boundaries: chunk N may end in `<thi` and chunk N+1
//! begin with `nk>...`. It buffers the ambiguous prefix and only commits
//! to either "this was plain text" or "this was a tag" once enough input
//! has arrived to decide.
//!
//! Behaviour:
//! - Recognises `<think>`, `<thinking>`, `<reasoning>`, `<thought>`
//!   (case-insensitive)
//!   and their corresponding closing tags. Other tags (e.g. `<b>`) pass
//!   through untouched — this is a reasoning filter, not an HTML sanitiser.
//! - Handles nested same-name blocks via a depth counter.
//! - Accepts attributes inside the opening tag (`<thinking attr="x">`).
//! - Self-closing form (`<think/>`) is stripped with no content drop.
//! - An unclosed tag eats to the end of the stream (`v0.9.0` choice — we
//!   would rather hide a runaway reasoning tail than leak it; the next
//!   stream resets the filter and recovers).

/// State of the filter's parse.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FilterState {
    /// Default: characters pass through to output.
    Text,
    /// Saw `<` in Text — accumulating until we know if it starts a tag.
    /// `pending` holds the chars including the `<`.
    MaybeOpenTag,
    /// Inside a `<think>`/`<reasoning>`/`<thinking>` block — drop chars.
    /// `depth` is 1 for an un-nested block, incremented for each nested
    /// same-name open we see.
    InThinking { depth: u32 },
    /// Inside a thinking block, saw `<` — accumulating to decide if it is
    /// a same-name open (depth++), a close (depth--), or neither (drop).
    MaybeCloseTag { depth: u32 },
}

/// The longest tag prefix we ever buffer in MaybeOpenTag / MaybeCloseTag
/// before giving up and flushing as plain text. `</thinking>` is 11 chars,
/// but an opening tag may legitimately carry attributes (`<thinking
/// foo="bar">`), so the cap is generous: 256 bytes accommodates realistic
/// attribute content while still bounding memory against an adversarial
/// stream that keeps a `<` open indefinitely.
const MAX_TAG_BUFFER: usize = 256;

/// The tracked reasoning tag names, lowercase.
const TAG_NAMES: &[&str] = &["think", "thinking", "reasoning", "thought"];

#[derive(Debug)]
pub struct ReasoningFilter {
    state: FilterState,
    /// Buffer for ambiguous tag prefixes (MaybeOpenTag, MaybeCloseTag).
    pending: String,
    /// v0.9.3 — accumulated reasoning content for end-of-stream emission.
    /// Drained via [`ReasoningFilter::take_captured`]. Multiple
    /// `<think>…</think>` blocks within a single stream are joined with
    /// `\n` for downstream rendering as a single
    /// `TurnElement::Thinking { body, … }`.
    captured: String,
    /// v0.9.3 — tracks whether the most recent reasoning block has been
    /// closed, so the NEXT block's content is preceded by `\n` in
    /// `captured`. Starts `true` (no prior block to separate from).
    prev_block_committed: bool,
    /// #1129 — set when [`ReasoningFilter::take_captured_delta`] has already
    /// drained non-empty reasoning for THIS stream. A streaming consumer
    /// (the protocol sink) drains after every chunk, so `captured` is
    /// normally empty at the moment a second `<think>` block opens and the
    /// newline block separator below would be lost. This remembers that content
    /// was produced. The TUI never calls the delta drain, so it stays
    /// `false` there and the separator condition is bit-for-bit the one that
    /// shipped.
    captured_any: bool,
}

impl Default for ReasoningFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl ReasoningFilter {
    pub fn new() -> Self {
        Self {
            state: FilterState::Text,
            pending: String::new(),
            captured: String::new(),
            prev_block_committed: true,
            captured_any: false,
        }
    }

    /// v0.9.3 — drain the accumulated reasoning content. Called by the
    /// protocol bridge at assistant `StreamEnd` to emit
    /// `TurnElement::Thinking { body: take_captured(), … }`. After a drain
    /// the buffer is empty and the next captured block starts fresh
    /// (no leading `\n`).
    pub fn take_captured(&mut self) -> String {
        self.prev_block_committed = true;
        self.captured_any = false;
        std::mem::take(&mut self.captured)
    }

    /// #1129 — drain the reasoning captured SO FAR without ending the block.
    ///
    /// [`Self::take_captured`] is an end-of-stream drain: it also declares
    /// the multi-block accumulator finished, so the next block starts a
    /// fresh body. A streaming consumer needs the opposite — drain after
    /// every chunk to forward reasoning incrementally, while the block
    /// bookkeeping (and therefore the newline separator between two blocks
    /// in one turn) carries on. Concatenating every delta of a stream yields
    /// exactly what a single `take_captured` at the end would have.
    pub fn take_captured_delta(&mut self) -> String {
        let drained = std::mem::take(&mut self.captured);
        self.captured_any |= !drained.is_empty();
        drained
    }

    /// Process the next chunk of streamed text and return the user-visible
    /// substring (with reasoning tags + content stripped).
    pub fn process(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        for ch in chunk.chars() {
            self.feed_char(ch, &mut out);
        }
        out
    }

    /// Reset the filter to its initial state. Call at turn boundaries
    /// (`StreamStart`) so a leftover pending buffer from a previous
    /// stream cannot leak into a new one.
    ///
    /// v0.9.3 — also clears the captured-reasoning accumulator, so a
    /// cancelled stream's in-flight reasoning cannot leak forward into
    /// the next turn.
    pub fn reset(&mut self) {
        self.state = FilterState::Text;
        self.pending.clear();
        self.captured.clear();
        self.prev_block_committed = true;
        self.captured_any = false;
    }

    fn feed_char(&mut self, ch: char, out: &mut String) {
        match self.state.clone() {
            FilterState::Text => {
                if ch == '<' {
                    self.state = FilterState::MaybeOpenTag;
                    self.pending.clear();
                    self.pending.push(ch);
                } else {
                    out.push(ch);
                }
            }
            FilterState::MaybeOpenTag => {
                self.pending.push(ch);
                match classify_open(&self.pending) {
                    OpenClass::CompleteOpen { self_closing } => {
                        // `<think>` or `<think/>` or `<thinking attr="x">`.
                        self.pending.clear();
                        if self_closing {
                            // `<think/>` — nothing to drop, return to Text.
                            self.state = FilterState::Text;
                        } else {
                            // v0.9.3 — entering a fresh reasoning block.
                            // If a prior block was committed AND we already
                            // have captured content, separate the two with
                            // a single `\n` so the downstream Thinking body
                            // reads as a multi-block transcript.
                            if self.prev_block_committed
                                && (!self.captured.is_empty() || self.captured_any)
                            {
                                self.captured.push('\n');
                            }
                            self.prev_block_committed = false;
                            self.state = FilterState::InThinking { depth: 1 };
                        }
                    }
                    OpenClass::Prefix => {
                        // Keep accumulating, unless we've hit the cap (a
                        // pathological run of "<thinkingxxxxxx..." that
                        // happens to share the prefix). The cap keeps
                        // memory bounded across an adversarial stream.
                        if self.pending.len() >= MAX_TAG_BUFFER {
                            // Flush as plain text and resume Text scanning.
                            // Re-feed the last char so a `<` in the
                            // overflow can still start a new tag check.
                            self.flush_pending_as_text(out);
                            self.state = FilterState::Text;
                        }
                    }
                    OpenClass::NotATag => {
                        // The accumulated string was never going to be a
                        // tag — flush it as plain text. The final char may
                        // itself be `<`, which can start a new tag check.
                        self.flush_pending_as_text(out);
                        self.state = FilterState::Text;
                        // Re-scan the trailing `<` we just flushed.
                        if let Some(last) = out.pop() {
                            if last == '<' {
                                self.state = FilterState::MaybeOpenTag;
                                self.pending.push('<');
                            } else {
                                out.push(last);
                            }
                        }
                    }
                }
            }
            FilterState::InThinking { depth } => {
                if ch == '<' {
                    // Don't push `<` yet — it might be the start of
                    // `</think>` (close) or `<think>` (nested open). The
                    // MaybeCloseTag arm decides and routes `pending`'s
                    // chars into `captured` on the not-a-tag / overflow
                    // branches.
                    self.state = FilterState::MaybeCloseTag { depth };
                    self.pending.clear();
                    self.pending.push(ch);
                } else {
                    // v0.9.3 — capture the reasoning content char. The
                    // existing strip path simply dropped it.
                    self.captured.push(ch);
                }
            }
            FilterState::MaybeCloseTag { depth } => {
                self.pending.push(ch);
                match classify_inside(&self.pending) {
                    InsideClass::CompleteClose => {
                        // `</think>` — pop one level of depth. The
                        // `pending` was the close tag itself (never
                        // reasoning content), so it is discarded.
                        self.pending.clear();
                        if depth <= 1 {
                            // v0.9.3 — the outermost reasoning block just
                            // closed; mark committed so a subsequent open
                            // block prepends `\n` to keep blocks readable
                            // in the captured body.
                            self.prev_block_committed = true;
                            self.state = FilterState::Text;
                        } else {
                            self.state = FilterState::InThinking { depth: depth - 1 };
                        }
                    }
                    InsideClass::CompleteOpen { self_closing } => {
                        // Nested `<think>` inside a `<think>` — depth++.
                        // The nested tag itself is not reasoning content,
                        // so `pending` is discarded; we do NOT prepend `\n`
                        // because the captured stream is continuous within
                        // the outer block.
                        self.pending.clear();
                        if self_closing {
                            // Nested `<think/>` is a no-op for depth.
                            self.state = FilterState::InThinking { depth };
                        } else {
                            self.state = FilterState::InThinking { depth: depth + 1 };
                        }
                    }
                    InsideClass::Prefix => {
                        if self.pending.len() >= MAX_TAG_BUFFER {
                            // v0.9.3 — the buffered chars were reasoning
                            // content that happened to start with `<` and
                            // overflowed without resolving. Capture them
                            // before dropping so they aren't lost from the
                            // emitted Thinking body. The previous strip
                            // path simply discarded them silently.
                            self.captured.push_str(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::InThinking { depth };
                        }
                    }
                    InsideClass::NotATag => {
                        // Some other tag-like content inside the reasoning
                        // block. v0.9.3 — those chars were reasoning
                        // content; capture all but the trailing char,
                        // which may itself be `<` and start a new
                        // close-tag check. (Mirrors the existing strip
                        // path's re-scan of a trailing `<`.)
                        let last = self.pending.chars().last();
                        let head_len = self.pending.len() - last.map(|c| c.len_utf8()).unwrap_or(0);
                        // Push the head (everything except the last char)
                        // into captured before discarding pending.
                        self.captured.push_str(&self.pending[..head_len]);
                        self.pending.clear();
                        if last == Some('<') {
                            self.state = FilterState::MaybeCloseTag { depth };
                            self.pending.push('<');
                        } else {
                            // The trailing char was reasoning content
                            // too (not a `<`) — capture it as well.
                            if let Some(c) = last {
                                self.captured.push(c);
                            }
                            self.state = FilterState::InThinking { depth };
                        }
                    }
                }
            }
        }
    }

    /// Flush the MaybeOpenTag buffer as plain output text (it turned out
    /// not to be a tag). Caller restores state separately.
    fn flush_pending_as_text(&mut self, out: &mut String) {
        out.push_str(&self.pending);
        self.pending.clear();
    }
}

// ── Tag classifiers ──────────────────────────────────────────────────────

/// What an accumulated MaybeOpenTag buffer means.
#[derive(Debug, PartialEq, Eq)]
enum OpenClass {
    /// `<name>` or `<name attr=...>` or `<name/>` — a complete recognised
    /// opening tag.
    CompleteOpen { self_closing: bool },
    /// The buffer is still a viable prefix of a recognised opening tag;
    /// keep accumulating.
    Prefix,
    /// The buffer is definitively NOT a recognised opening tag — flush as
    /// plain text.
    NotATag,
}

/// What an accumulated MaybeCloseTag buffer means (we are inside a
/// reasoning block, scanning for `</name>` or a nested `<name>`).
#[derive(Debug, PartialEq, Eq)]
enum InsideClass {
    /// `</name>` — close one level of depth.
    CompleteClose,
    /// Nested `<name>` or `<name/>` — open one more level.
    CompleteOpen { self_closing: bool },
    /// Still a viable prefix of either; keep accumulating.
    Prefix,
    /// Neither — drop and resume InThinking.
    NotATag,
}

/// Classify a MaybeOpenTag buffer. The buffer always starts with `<`.
fn classify_open(buf: &str) -> OpenClass {
    debug_assert!(buf.starts_with('<'));
    let body = &buf[1..];

    // `<` alone — could be anything yet.
    if body.is_empty() {
        return OpenClass::Prefix;
    }
    // `</...` is a close tag, never an open — reject early. (We can only
    // get here from FilterState::Text where no block is open, so a stray
    // `</think>` is just plain text.)
    if body.starts_with('/') {
        return OpenClass::NotATag;
    }

    classify_tag_body(body, /* expect_close = */ false).map_or(OpenClass::NotATag, |class| {
        match class {
            TagClass::Prefix => OpenClass::Prefix,
            TagClass::Complete { self_closing } => OpenClass::CompleteOpen { self_closing },
        }
    })
}

/// Classify a MaybeCloseTag buffer (inside a reasoning block). The buffer
/// always starts with `<`. The buffer is either a closing tag for the
/// current block, a nested opening tag, or unrelated tag-ish text.
fn classify_inside(buf: &str) -> InsideClass {
    debug_assert!(buf.starts_with('<'));
    let body = &buf[1..];

    if body.is_empty() {
        return InsideClass::Prefix;
    }

    if let Some(close_body) = body.strip_prefix('/') {
        // `</...`
        return match classify_tag_body(close_body, /* expect_close = */ true) {
            Some(TagClass::Prefix) => InsideClass::Prefix,
            Some(TagClass::Complete { .. }) => InsideClass::CompleteClose,
            None => InsideClass::NotATag,
        };
    }
    // A `</` is also still a prefix of either — only confirmed when the
    // next char arrives.
    if buf == "<" {
        return InsideClass::Prefix;
    }

    match classify_tag_body(body, /* expect_close = */ false) {
        Some(TagClass::Prefix) => InsideClass::Prefix,
        Some(TagClass::Complete { self_closing }) => InsideClass::CompleteOpen { self_closing },
        None => InsideClass::NotATag,
    }
}

#[derive(Debug)]
enum TagClass {
    /// Could still complete into a recognised tag — keep buffering.
    Prefix,
    /// A complete recognised tag (open or close, depending on caller).
    Complete { self_closing: bool },
}

/// Inspect the substring after the leading `<` (and optional `/`). Returns
/// `Some(Prefix)` if the body could still grow into a recognised tag,
/// `Some(Complete)` if the body IS a complete recognised tag, and `None`
/// if it definitively isn't.
fn classify_tag_body(body: &str, expect_close: bool) -> Option<TagClass> {
    // Walk the body char-by-char. Pull out the tag-name prefix and decide.
    // A recognised tag name is one of TAG_NAMES (case-insensitive). After
    // the name, the only legal next chars are `>` (closes the tag), `/`
    // (followed by `>` for self-closing), or whitespace (followed by
    // attributes up to a closing `>`).

    let mut name_end = 0usize;
    let mut chars = body.char_indices();
    let mut after_name: Option<(usize, char)> = None;
    for (idx, ch) in chars.by_ref() {
        if ch.is_ascii_alphabetic() {
            name_end = idx + ch.len_utf8();
        } else {
            after_name = Some((idx, ch));
            break;
        }
    }
    let name = &body[..name_end];
    let name_lower = name.to_ascii_lowercase();

    // If we haven't seen the terminator yet, decide whether the partial
    // name could still match a tracked tag.
    let Some((term_idx, term_ch)) = after_name else {
        // Whole body is alphabetic — still a prefix of any tag whose name
        // starts with this string.
        if name_lower.is_empty() {
            return Some(TagClass::Prefix);
        }
        // Is the name itself a recognised tag name (no terminator yet)?
        // Could still be (e.g. user might type `<think` then ` `). Keep
        // buffering.
        if TAG_NAMES.iter().any(|t| t.starts_with(&name_lower[..])) {
            return Some(TagClass::Prefix);
        }
        // The name does not match any tracked tag's prefix.
        // Special-case closes: `</a` where `a` isn't a tag-name prefix
        // could still be a tag we don't care about — but classify says
        // NotATag and the caller treats it as plain text inside Text or
        // drops it inside InThinking. Either way it's "not a reasoning
        // tag" → None.
        return None;
    };

    // We have a name and a terminator-ish char. The name must exactly
    // match a tracked tag name.
    if !TAG_NAMES.iter().any(|t| *t == name_lower) {
        return None;
    }

    // The body after the name is `body[term_idx..]`, starting with
    // `term_ch`. The terminator must be `>`, `/`, or whitespace, else
    // this isn't a real tag (e.g. `<thinking-other>` is not us).
    match term_ch {
        '>' => Some(TagClass::Complete {
            self_closing: false,
        }),
        '/' => {
            // Self-closing form: must be `/>`. If the body ends at `/`,
            // we're still a prefix.
            let after = &body[term_idx + 1..];
            if after.is_empty() {
                Some(TagClass::Prefix)
            } else if after.starts_with('>') {
                if expect_close {
                    // `</think/>` is malformed — treat as not-a-tag.
                    None
                } else {
                    Some(TagClass::Complete { self_closing: true })
                }
            } else {
                None
            }
        }
        ch if ch.is_ascii_whitespace() => {
            if expect_close {
                // `</think foo>` is malformed in HTML but some emitters
                // do it. Look for the closing `>` ignoring contents.
                find_close(&body[term_idx..])
            } else {
                // Opening tag with attributes — scan forward to `>`.
                find_close(&body[term_idx..])
            }
        }
        _ => None,
    }
}

/// Given a slice that starts with whitespace (or similar) inside an open
/// tag, find the closing `>` and report Complete. If we haven't seen `>`
/// yet, report Prefix. If we see something pathological (newline before
/// `>`? — we still accept; HTML allows it), keep scanning.
fn find_close(rest: &str) -> Option<TagClass> {
    // We've already consumed the tag name + the first attribute-area char.
    // Scan for `>` (or `/>` for self-closing in attr area).
    for (idx, ch) in rest.char_indices() {
        match ch {
            '>' => {
                return Some(TagClass::Complete {
                    self_closing: false,
                });
            }
            '/' => {
                // `... />` ?
                let next = rest[idx + 1..].chars().next();
                match next {
                    Some('>') => {
                        return Some(TagClass::Complete { self_closing: true });
                    }
                    Some(_) => {
                        // `/` mid-attribute — keep scanning.
                        continue;
                    }
                    None => return Some(TagClass::Prefix),
                }
            }
            _ => continue,
        }
    }
    // Reached the end of the buffer without a `>` — still a prefix.
    Some(TagClass::Prefix)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> String {
        let mut filter = ReasoningFilter::new();
        let mut out = String::new();
        for c in chunks {
            out.push_str(&filter.process(c));
        }
        out
    }

    #[test]
    fn passes_through_text_with_no_tags() {
        assert_eq!(run(&["Hello, world!"]), "Hello, world!");
    }

    #[test]
    fn strips_simple_think_block() {
        assert_eq!(run(&["Hello <think>foo</think> world"]), "Hello  world");
    }

    #[test]
    fn strips_simple_reasoning_block() {
        assert_eq!(run(&["<reasoning>x</reasoning>visible"]), "visible");
    }

    #[test]
    fn strips_thinking_variant() {
        assert_eq!(run(&["<thinking>x</thinking>visible"]), "visible");
    }

    #[test]
    fn case_insensitive_open_close() {
        assert_eq!(run(&["<THINK>x</think>"]), "");
        assert_eq!(run(&["<Reasoning>x</REASONING>"]), "");
        assert_eq!(run(&["<Thinking>x</Thinking>tail"]), "tail");
    }

    #[test]
    fn tag_split_across_chunks_open() {
        // The classic streaming hazard: open tag straddles a chunk
        // boundary. The filter must buffer "Hi <thi" without emitting it
        // and then suppress everything until the close on the next chunk.
        assert_eq!(run(&["Hi <thi", "nk>x</think> bye"]), "Hi  bye");
    }

    #[test]
    fn tag_split_across_chunks_close() {
        // The closing tag straddles a chunk boundary.
        assert_eq!(run(&["<think>foo</thi", "nk> bye"]), " bye");
    }

    #[test]
    fn nested_think_blocks_handled() {
        assert_eq!(
            run(&["<think>a<think>b</think>c</think>visible"]),
            "visible"
        );
    }

    #[test]
    fn unclosed_tag_eats_to_end() {
        // For v0.9.0 an unclosed reasoning block eats to end-of-stream.
        // Recovery happens at the next StreamStart via reset().
        assert_eq!(run(&["<think>never closes"]), "");
    }

    #[test]
    fn unknown_tags_pass_through() {
        assert_eq!(run(&["<b>bold</b>"]), "<b>bold</b>");
        assert_eq!(
            run(&["before <span class=\"x\">mid</span> after"]),
            "before <span class=\"x\">mid</span> after"
        );
    }

    #[test]
    fn partial_tag_at_end_of_chunk_buffered() {
        // A trailing `<` that turns out to be plain text must eventually
        // be re-emitted once enough disambiguating chars arrive.
        assert_eq!(run(&["Hi <", " world"]), "Hi < world");
    }

    #[test]
    fn reset_clears_pending_buffer() {
        let mut filter = ReasoningFilter::new();
        // Begin an open-tag prefix...
        let out1 = filter.process("Hi <thi");
        assert_eq!(out1, "Hi "); // The `<thi` is buffered, not emitted.
        // ...then reset (e.g. a new StreamStart fires).
        filter.reset();
        // Next chunk starts fresh — the buffered `<thi` must be dropped,
        // and `nk>x</think>` becomes a complete reasoning block on its
        // own, fully stripped.
        let out2 = filter.process("<think>x</think>after");
        assert_eq!(out2, "after");
        // v0.9.3 — reset also drains the captured-reasoning accumulator.
        // The block we just processed captured "x"; consuming it now and
        // then resetting must leave the capture buffer empty afterwards.
        assert_eq!(filter.take_captured(), "x");
    }

    // ── v0.9.3 W1.2 — captured reasoning accumulator ─────────────────

    #[test]
    fn capture_buffer_accumulates_thinking_content_v093() {
        let mut filter = ReasoningFilter::new();
        let visible = filter.process("Some prefix <thinking>I should consider X.</thinking>");
        assert_eq!(visible, "Some prefix ");
        assert_eq!(filter.take_captured(), "I should consider X.");
    }

    #[test]
    fn capture_buffer_concatenates_multiple_blocks_v093() {
        let mut filter = ReasoningFilter::new();
        let _ = filter.process("<think>A</think>between<think>B</think>");
        // Multiple captured blocks join with newline.
        assert_eq!(filter.take_captured(), "A\nB");
    }

    #[test]
    fn capture_buffer_handles_cross_chunk_tags_v093() {
        let mut filter = ReasoningFilter::new();
        let _ = filter.process("<think>foo</thi");
        let _ = filter.process("nk>after");
        assert_eq!(filter.take_captured(), "foo");
    }

    #[test]
    fn take_captured_drains_and_clears_v093() {
        let mut filter = ReasoningFilter::new();
        let _ = filter.process("<thinking>X</thinking>");
        assert_eq!(filter.take_captured(), "X");
        // Second call returns empty — the buffer was drained.
        assert_eq!(filter.take_captured(), "");
    }

    #[test]
    fn capture_buffer_empty_when_no_reasoning_v093() {
        let mut filter = ReasoningFilter::new();
        let _ = filter.process("plain text no tags");
        assert_eq!(filter.take_captured(), "");
    }

    #[test]
    fn reset_clears_captured_v093() {
        // v1.3 SPEC §1 test contract: reset() drains the capture buffer
        // so a cancelled stream's reasoning cannot leak into the next.
        let mut filter = ReasoningFilter::new();
        let _ = filter.process("<think>leak me</think>");
        filter.reset();
        assert_eq!(filter.take_captured(), "");
    }

    // ── Bonus regression tests ───────────────────────────────────────

    #[test]
    fn self_closing_think_strips_with_no_content() {
        assert_eq!(run(&["before<think/>after"]), "beforeafter");
    }

    #[test]
    fn malformed_open_tag_with_attributes() {
        // Some emitters add (non-standard) attributes. We accept anything
        // up to the next `>`.
        assert_eq!(run(&["<thinking attr=\"oops\">x</thinking>tail"]), "tail");
    }

    #[test]
    fn stray_open_bracket_followed_by_alpha_non_tag() {
        // `<xy>` is not a reasoning tag and must pass through.
        assert_eq!(run(&["a <xy>b</xy> c"]), "a <xy>b</xy> c");
    }

    #[test]
    fn consecutive_reasoning_blocks() {
        assert_eq!(run(&["a<think>1</think>b<reasoning>2</reasoning>c"]), "abc");
    }

    #[test]
    fn close_tag_outside_block_is_plain_text() {
        // `</think>` appearing in plain text (no open) — pass through. We
        // treat it as plain text because it isn't a recognised opening
        // tag and we are not in a reasoning block.
        assert_eq!(run(&["plain </think> text"]), "plain </think> text");
    }

    #[test]
    fn many_small_chunks_simulating_token_stream() {
        // The real adversary: every char arrives separately.
        let s = "Hi <think>secret</think> world";
        let mut filter = ReasoningFilter::new();
        let mut out = String::new();
        for ch in s.chars() {
            out.push_str(&filter.process(&ch.to_string()));
        }
        assert_eq!(out, "Hi  world");
    }

    #[test]
    fn tag_name_prefix_then_unrelated() {
        // `<thi` could be the start of `<think>`, but if it resolves to
        // `<thigh>` (not a tracked tag), flush as plain text.
        assert_eq!(run(&["<thigh>x</thigh>"]), "<thigh>x</thigh>");
    }

    #[test]
    fn split_thinking_variant_across_chunks() {
        // Hardest split: `<thinking` is itself a prefix of `<thinking>` AND
        // diverges from `<think>` only at char 7. Streaming this in
        // single-char chunks must end with the whole block stripped.
        assert_eq!(
            run(&[
                "<", "t", "h", "i", "n", "k", "i", "n", "g", ">", "x", "<", "/", "t", "h", "i",
                "n", "k", "i", "n", "g", ">", "Y"
            ]),
            "Y"
        );
    }
    // ── #1129 — streaming drain for the JSON-stream protocol sink ─────────

    /// Drain per chunk (what `ProtocolSink` does) and drain once at the end
    /// (what the TUI does) must produce the SAME reasoning body. If they can
    /// diverge, a Desktop host and the local TUI show different reasoning for
    /// the same stream.
    #[test]
    fn per_chunk_drain_concatenates_to_the_end_of_stream_drain_1129() {
        let chunks = [
            "intro <thi",
            "nk>first ",
            "half</think> mid <reasoning>second</reasoning> tail",
        ];

        let mut streaming = ReasoningFilter::new();
        let mut streamed = String::new();
        let mut visible_streamed = String::new();
        for c in chunks {
            visible_streamed.push_str(&streaming.process(c));
            streamed.push_str(&streaming.take_captured_delta());
        }
        streamed.push_str(&streaming.take_captured_delta());

        let mut batched = ReasoningFilter::new();
        let mut visible_batched = String::new();
        for c in chunks {
            visible_batched.push_str(&batched.process(c));
        }
        let batched_body = batched.take_captured();

        assert_eq!(streamed, batched_body);
        assert_eq!(visible_streamed, visible_batched);
        assert_eq!(batched_body, "first half\nsecond");
    }

    /// The per-chunk drain must not lose the block separator. `captured` is
    /// empty at the moment the second block opens (the first was already
    /// drained), so the naive implementation joins the two bodies with no
    /// break at all.
    #[test]
    fn per_chunk_drain_keeps_the_block_separator_1129() {
        let mut f = ReasoningFilter::new();
        let mut body = String::new();
        f.process("<think>one</think>");
        body.push_str(&f.take_captured_delta());
        f.process("mid<think>two</think>end");
        body.push_str(&f.take_captured_delta());
        assert_eq!(body, "one\ntwo");
    }

    /// The separator is content-gated, not block-gated: an EMPTY first block
    /// must not put a leading newline in front of the second block's body.
    /// Asserted in both drain modes so the streaming path cannot drift from
    /// the shipped TUI behaviour.
    #[test]
    fn an_empty_first_block_adds_no_separator_in_either_drain_mode_1129() {
        let mut batched = ReasoningFilter::new();
        batched.process("<think></think>a<think>body</think>b");
        assert_eq!(batched.take_captured(), "body");

        let mut streaming = ReasoningFilter::new();
        let mut body = String::new();
        streaming.process("<think></think>a");
        body.push_str(&streaming.take_captured_delta());
        streaming.process("<think>body</think>b");
        body.push_str(&streaming.take_captured_delta());
        assert_eq!(body, "body");
    }

    /// A delta drain does NOT declare the block finished: reasoning that
    /// straddles the drain keeps accumulating into the same body with no
    /// spurious separator injected mid-block.
    #[test]
    fn a_delta_drain_does_not_end_the_open_block_1129() {
        let mut f = ReasoningFilter::new();
        let mut body = String::new();
        f.process("<think>first");
        body.push_str(&f.take_captured_delta());
        f.process(" second</think>visible");
        body.push_str(&f.take_captured_delta());
        assert_eq!(body, "first second");
    }

    /// `take_captured` (the TUI's end-of-stream drain) still resets the
    /// multi-block accumulator, so a following block starts a fresh body
    /// with no leading separator — the behaviour that shipped.
    #[test]
    fn take_captured_still_resets_the_accumulator_1129() {
        let mut f = ReasoningFilter::new();
        f.process("<think>one</think>");
        assert_eq!(f.take_captured(), "one");
        f.process("<think>two</think>");
        assert_eq!(f.take_captured(), "two");
    }

    /// #1129 asked for EVERY spelling; `thought` was missing from
    /// `TAG_NAMES`, so a model emitting `<thought>` leaked it verbatim.
    #[test]
    fn thought_is_a_recognised_spelling_1129() {
        let mut f = ReasoningFilter::new();
        let visible = f.process("before<thought>musing</thought>after");
        assert_eq!(visible, "beforeafter");
        assert_eq!(f.take_captured(), "musing");
    }

    /// The hazard the `thought` addition introduces: `thou`/`though` is now
    /// a live tag-name prefix, so a tag that merely STARTS like one must
    /// still reach the user byte for byte, and the word in prose untouched.
    #[test]
    fn thought_prefix_does_not_swallow_lookalikes_1129() {
        let mut f = ReasoningFilter::new();
        let src = "I thought <thoughtful>x</thoughtful> though <tho> ok";
        let visible = f.process(src);
        assert_eq!(visible, src);
        assert_eq!(f.take_captured(), "");
    }
}
