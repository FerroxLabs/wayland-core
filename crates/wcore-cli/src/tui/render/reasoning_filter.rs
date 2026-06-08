//! Streaming reasoning-tag filter.
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
//! - Recognises `<think>`, `<thinking>`, `<reasoning>` (case-insensitive)
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
const TAG_NAMES: &[&str] = &["think", "thinking", "reasoning"];

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
        }
    }

    /// v0.9.3 — drain the accumulated reasoning content. Called by the
    /// protocol bridge at assistant `StreamEnd` to emit
    /// `TurnElement::Thinking { body: take_captured(), … }`. After a drain
    /// the buffer is empty and the next captured block starts fresh
    /// (no leading `\n`).
    pub fn take_captured(&mut self) -> String {
        self.prev_block_committed = true;
        std::mem::take(&mut self.captured)
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
                            if self.prev_block_committed && !self.captured.is_empty() {
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

// ── S21: collapsed reasoning projection ───────────────────────────────────
//
// v0.9.2 W7 (SPEC §3 S21, variant A). A captured reasoning block renders
// collapsed-by-default as a single `▶ Thought: <title> · Ns · N tok`
// line. The user toggles it open (Tab to focus the block, Enter to
// expand) — keyed by turn index in `App::reasoning_expanded`. When
// expanded the marker flips to `▼` and the body follows on subsequent
// wrapped lines.
//
// The `reasoning_filter` STRIPS reasoning tags from the live streaming
// buffer (so reasoning never leaks into prose); this projection renders
// reasoning that the engine surfaced *as* a discrete block (e.g. an
// Anthropic `thinking` content block or a provider summary), which is a
// separate, deliberate-to-show payload.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::theme::Theme;

/// Max chars for the one-line collapsed title before we ellipsize.
const REASONING_TITLE_MAX: usize = 50;

/// Extract a short title from a reasoning summary: the first **bold**
/// span if the summary opens with one (`**Title** …`), else the first
/// sentence (up to the first `.`/`!`/`?`), else the leading text.
/// Whitespace-collapsed and truncated to [`REASONING_TITLE_MAX`] chars
/// with a trailing `…` when cut.
pub fn reasoning_title(summary: &str) -> String {
    let trimmed = summary.trim();
    // First bold span: `**...**` at the very start.
    let raw = if let Some(rest) = trimmed.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            rest[..end].trim().to_string()
        } else {
            first_sentence(trimmed)
        }
    } else {
        first_sentence(trimmed)
    };
    // Collapse internal whitespace (incl. newlines) to single spaces.
    let collapsed: String = {
        let mut s = String::with_capacity(raw.len());
        let mut in_ws = false;
        for ch in raw.chars() {
            if ch.is_whitespace() {
                if !in_ws {
                    s.push(' ');
                    in_ws = true;
                }
            } else {
                s.push(ch);
                in_ws = false;
            }
        }
        s.trim().to_string()
    };
    if collapsed.chars().count() > REASONING_TITLE_MAX {
        let head: String = collapsed
            .chars()
            .take(REASONING_TITLE_MAX.saturating_sub(1))
            .collect();
        format!("{head}…")
    } else {
        collapsed
    }
}

/// The first sentence of `s` — text up to (and excluding) the first
/// sentence-terminator (`.`/`!`/`?`). Falls back to the whole string if
/// none is present.
fn first_sentence(s: &str) -> String {
    match s.find(['.', '!', '?']) {
        Some(ix) => s[..ix].to_string(),
        None => s.to_string(),
    }
}

/// Project a reasoning block to its renderable lines, honoring the
/// per-turn expand state.
///
/// * Collapsed (`expanded == false`): one line
///   `▶ Thought: <title> · Ns · N tok`. The duration / token counts are
///   omitted when zero so a block with no timing reads cleanly.
/// * Expanded (`expanded == true`): a `▼ Thought: <title>` header line
///   followed by the wrapped body (one `Line` per source line of the
///   reasoning summary), indented two spaces to sit under the header.
///
/// The marker + "Thought:" label are `text_muted`; the title is
/// `text_dim`; the timing meta is `text_muted`. Reasoning is ancillary,
/// so nothing here uses the brand accent.
pub fn reasoning_collapsed_lines(
    summary: &str,
    secs: u64,
    tokens: u64,
    expanded: bool,
) -> Vec<Line<'static>> {
    reasoning_collapsed_lines_themed(summary, secs, tokens, expanded, &Theme::detect())
}

/// [`reasoning_collapsed_lines`] with an explicit theme (so callers that
/// already hold the resolved `Theme` don't re-detect, and tests can pin
/// a known palette).
pub fn reasoning_collapsed_lines_themed(
    summary: &str,
    secs: u64,
    tokens: u64,
    expanded: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let title = reasoning_title(summary);
    let marker = if expanded { "▼" } else { "▶" };
    let muted = Style::default().fg(theme.text_muted);
    let dim = Style::default().fg(theme.text_dim);

    let mut header: Vec<Span<'static>> = vec![
        Span::styled(format!("{marker} "), muted),
        Span::styled("Thought: ".to_string(), muted.add_modifier(Modifier::BOLD)),
        Span::styled(title, dim),
    ];
    // Timing meta — only the parts that carry information.
    let mut meta = String::new();
    if secs > 0 {
        meta.push_str(&format!(" · {secs}s"));
    }
    if tokens > 0 {
        meta.push_str(&format!(" · {tokens} tok"));
    }
    if !meta.is_empty() {
        header.push(Span::styled(meta, muted));
    }

    let mut out = vec![Line::from(header)];
    if expanded {
        for src_line in summary.lines() {
            out.push(Line::from(vec![Span::styled(format!("  {src_line}"), dim)]));
        }
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────────

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

    // ── v0.9.2 W7 (S21) — collapsed reasoning projection ───────────────

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn reasoning_title_takes_first_bold_span_v092() {
        let title = reasoning_title("**Plan the refactor** then we proceed.");
        assert_eq!(title, "Plan the refactor");
    }

    #[test]
    fn reasoning_title_falls_back_to_first_sentence_v092() {
        let title = reasoning_title("Considering the edge cases. Then the happy path.");
        assert_eq!(title, "Considering the edge cases");
    }

    #[test]
    fn reasoning_title_truncates_long_titles_v092() {
        let long = "a".repeat(120);
        let title = reasoning_title(&long);
        assert!(
            title.chars().count() <= REASONING_TITLE_MAX,
            "title not truncated: {} chars",
            title.chars().count()
        );
        assert!(
            title.ends_with('…'),
            "truncated title must end with ellipsis"
        );
    }

    #[test]
    fn reasoning_collapsed_default_is_single_marker_line_v092() {
        let lines = reasoning_collapsed_lines_themed(
            "**Weighing options** in detail across the whole module.",
            4,
            128,
            /* expanded = */ false,
            &Theme::hearth(),
        );
        assert_eq!(lines.len(), 1, "collapsed reasoning must be one line");
        let text = line_text(&lines[0]);
        assert!(
            text.starts_with("▶ "),
            "collapsed marker must be ▶; got {text:?}"
        );
        assert!(
            text.contains("Thought: "),
            "missing Thought label; got {text:?}"
        );
        assert!(
            text.contains("Weighing options"),
            "missing title; got {text:?}"
        );
        assert!(text.contains("· 4s"), "missing seconds meta; got {text:?}");
        assert!(
            text.contains("· 128 tok"),
            "missing token meta; got {text:?}"
        );
    }

    #[test]
    fn reasoning_expanded_shows_marker_flip_and_body_v092() {
        let summary = "First line of thought\nSecond line of thought";
        let lines = reasoning_collapsed_lines_themed(
            summary,
            0,
            0,
            /* expanded = */ true,
            &Theme::hearth(),
        );
        // Header + one line per source line.
        assert_eq!(lines.len(), 3, "expanded must be header + 2 body lines");
        let header = line_text(&lines[0]);
        assert!(
            header.starts_with("▼ "),
            "expanded marker must be ▼; got {header:?}"
        );
        // No timing meta when both counts are zero.
        assert!(
            !header.contains(" · "),
            "zero-timing header must omit meta; got {header:?}"
        );
        assert!(line_text(&lines[1]).contains("First line of thought"));
        assert!(line_text(&lines[2]).contains("Second line of thought"));
    }

    /// The collapsed/expanded choice is driven by the per-turn flag the
    /// App stores in `reasoning_expanded` — model the lookup here (absent
    /// or `false` ⇒ collapsed) to lock the contract the render path uses.
    #[test]
    fn reasoning_expanded_map_semantics_v092() {
        let mut expanded: std::collections::HashMap<usize, bool> = Default::default();
        let summary = "Some reasoning here.";
        // Turn 0 absent ⇒ collapsed (one line).
        let collapsed = reasoning_collapsed_lines_themed(
            summary,
            0,
            0,
            expanded.get(&0).copied().unwrap_or(false),
            &Theme::hearth(),
        );
        assert_eq!(collapsed.len(), 1);
        // Toggle turn 0 open ⇒ expanded (header + body).
        expanded.insert(0, true);
        let open = reasoning_collapsed_lines_themed(
            summary,
            0,
            0,
            expanded.get(&0).copied().unwrap_or(false),
            &Theme::hearth(),
        );
        assert!(open.len() > 1, "expanded turn must render body");
    }
}
