//! Markdown → `Vec<Line<'static>>` rendering for the TUI transcript.
//!
//! Walks a [`pulldown_cmark::Parser`] event stream and lowers each event to
//! styled [`ratatui::text::Span`]s, flushing into [`Line`]s on paragraph /
//! list-item / heading / code-block / break boundaries. The returned tuple is:
//!
//! * `Vec<Line<'static>>` — the styled output the transcript can render
//!   directly with `Paragraph::new`.
//! * `Vec<String>` — every inline link URL the parser emitted, in source
//!   order. W3 D4 ("Sources" block) consumes this to render the footer of
//!   citation URLs; passing it back from the renderer keeps a single source
//!   of truth for what URLs the body referenced.
//!
//! ## Scope (v0.9.0 W2 C1)
//!
//! Supported markdown features (per C1 briefing):
//!
//! * `**bold**`, `*italic*` (`Modifier::BOLD` / `ITALIC`)
//! * Headings H1–H3 (bold + `theme.heading`); H4–H6 fall through as bold.
//! * Bullet lists (`-`/`*`) → `  • ` prefix
//! * Numbered lists → `  N. ` prefix (counter advances per `Item`)
//! * Inline code → orange fg + `theme.surface` bg
//! * Fenced code blocks → code style, language label dim on the opener line
//! * Inline links `[text](url)` → text in `theme.link`, URL appended as
//!   ` (url)` in dim style. URLs collected into the returned `Vec<String>`.
//! * Blockquote → `│ ` prefix in dim
//! * `SoftBreak` / `HardBreak` → flush current line
//!
//! ## Streaming
//!
//! For W2 streaming, `source` may be a partial markdown chunk (e.g. ending
//! mid-fence). The renderer renders whatever pulldown-cmark yields — the
//! safe-split logic that decides *where* to cut a chunk lives in W2 C5.
//! Practical consequences for C5 documented in commit notes.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::render::osc8;
use crate::tui::theme::Theme;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

/// Per-thread markdown render memo cap. The transcript re-renders every frame
/// (~30fps); completed turns' markdown is immutable, so re-parsing it each
/// frame is O(scrollback) waste that grows unbounded with conversation length
/// — the "molasses / 5 chars at a time" perf bug (see
/// `.planning/audits/2026-05-31-tui-perf-teardown.md`). This caps how many
/// distinct rendered blocks we retain; FIFO-evicted entries just re-render
/// once when next seen. 4096 covers very long sessions while bounding memory.
const MARKDOWN_CACHE_CAP: usize = 4096;

thread_local! {
    /// Thread-local render memo keyed by `(source, width, theme)`. The TUI
    /// render path runs on one thread; any off-thread caller (e.g. URL
    /// extraction at `StreamEnd`) gets its own independent cache. Correctness
    /// holds either way: every render input is folded into the key, so a hit
    /// can never surface stale output.
    static MARKDOWN_CACHE: RefCell<MarkdownMemo> = RefCell::new(MarkdownMemo::new());
}

/// FIFO-bounded map from a render key to rendered `(lines, urls)`.
struct MarkdownMemo {
    map: HashMap<u64, (Vec<Line<'static>>, Vec<String>)>,
    order: VecDeque<u64>,
}

impl MarkdownMemo {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: u64) -> Option<(Vec<Line<'static>>, Vec<String>)> {
        self.map.get(&key).cloned()
    }

    fn insert(&mut self, key: u64, value: (Vec<Line<'static>>, Vec<String>)) {
        // Only track ordering for genuinely new keys so a re-insert (which the
        // hot path never does, but be defensive) can't desync `order`/`map`.
        if self.map.insert(key, value).is_none() {
            self.order.push_back(key);
            if self.order.len() > MARKDOWN_CACHE_CAP
                && let Some(evicted) = self.order.pop_front()
            {
                self.map.remove(&evicted);
            }
        }
    }
}

/// Hash `(source, width, theme)` into the memo key. The theme is folded in so a
/// theme switch can never surface stale (wrong-colored) cached lines.
fn markdown_cache_key(source: &str, content_width: u16, theme: &Theme) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    content_width.hash(&mut h);
    hash_theme(theme, &mut h);
    h.finish()
}

/// Fold every themed color the renderer can use into the hasher. ratatui's
/// `Color` is `Hash`, so any palette change yields a different key. `pub(crate)`
/// so the tool-card formatter memo (`surfaces/workspace.rs`) folds the SAME
/// palette fingerprint into its key without duplicating the color list.
pub(crate) fn hash_theme(t: &Theme, h: &mut impl Hasher) {
    for color in [
        t.orange,
        t.orange_hover,
        t.orange_muted,
        t.orange_light,
        t.bg,
        t.surface,
        t.surface_elevated,
        t.surface_hover,
        t.border,
        t.text,
        t.text_dim,
        t.text_muted,
        t.text_running,
        t.success,
        t.warning,
        t.error,
        t.heading,
        t.link,
    ] {
        color.hash(h);
    }
}

/// Render a markdown source string into styled ratatui `Line`s + the list
/// of inline link URLs encountered in source order.
///
/// See the module-level docs for the supported feature set and streaming
/// semantics.
///
/// Delegates to [`render_markdown_with_width`] with `u16::MAX` so the
/// table renderer never triggers the v0.9.1.2 F11-followup bullet-list
/// fallback — callers that don't know the viewport width get the legacy
/// box-drawing layout for every table. Use [`render_markdown_with_width`]
/// when the caller has a real `content_width` and wants wide tables to
/// degrade gracefully (see Sean's screenshot: misaligned columns, raw
/// pipes wrapped onto separate rows).
pub fn render_markdown(source: &str, theme: &Theme) -> (Vec<Line<'static>>, Vec<String>) {
    render_markdown_with_width(source, theme, u16::MAX)
}

/// Render markdown with a viewport-width budget for tables.
///
/// When a markdown table's natural width (sum of column widths + pipe
/// separators) exceeds `content_width`, the renderer falls back to a
/// definition-list-style bullet rendering instead of clipping cells or
/// wrapping pipes onto separate rows. Tables that fit are unchanged —
/// the F11 box-drawing layout still wins for the common narrow case.
///
/// `content_width` is the number of terminal columns the rendered lines
/// may occupy. Callers should pass the *post-gutter* width (e.g. for the
/// 2-space assistant indent, pass `viewport_width - 2`). Passing
/// `u16::MAX` disables the fallback entirely.
///
/// ## Caching (2026-05-31 perf teardown)
///
/// This is the hot transcript-render entry point, called for every markdown
/// block every frame. The result is a pure function of `(source, theme,
/// content_width)`, so it is memoized in a per-thread [`MarkdownMemo`] keyed on
/// all three. Completed turns' markdown is immutable, so after the first frame
/// they are served from the cache (a clone) instead of re-parsed — turning the
/// per-frame cost from O(scrollback) markdown parsing into O(scrollback) memcpy.
/// The streaming tail also benefits: its `safe` prefix is stable between
/// safe-split points, so intervening frames hit the cache too. The memo cannot
/// go stale because every render input is part of the key.
pub fn render_markdown_with_width(
    source: &str,
    theme: &Theme,
    content_width: u16,
) -> (Vec<Line<'static>>, Vec<String>) {
    let key = markdown_cache_key(source, content_width, theme);
    if let Some(hit) = MARKDOWN_CACHE.with(|c| c.borrow().get(key)) {
        return hit;
    }
    let rendered = render_markdown_with_width_uncached(source, theme, content_width);
    MARKDOWN_CACHE.with(|c| c.borrow_mut().insert(key, rendered.clone()));
    rendered
}

/// Uncached markdown render. [`render_markdown_with_width`] is the cached entry
/// point the hot render path uses; this does the actual pulldown-cmark walk.
fn render_markdown_with_width_uncached(
    source: &str,
    theme: &Theme,
    content_width: u16,
) -> (Vec<Line<'static>>, Vec<String>) {
    // Pre-process the source so triple-backtick code blocks containing
    // triple-backtick examples render correctly. See
    // [`normalize_nested_fences`] for the algorithm; the pass is
    // idempotent so calling it twice is harmless.
    let normalized = normalize_nested_fences(source);
    let mut renderer = MarkdownRenderer::new(theme, content_width);
    // Enable the GFM-ish extensions the briefing requires. v0.9.1.2 F11
    // turns on tables (so the table arms below can flush real box-drawing
    // output) and strikethrough (cheap GFM bonus rendered via
    // `Modifier::CROSSED_OUT`).
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(&normalized, opts);
    for ev in parser {
        renderer.handle(ev);
    }
    renderer.finalize()
}

/// Upgrade outer fences so nested code blocks render correctly.
///
/// LLMs frequently emit triple-backtick code blocks that contain
/// triple-backtick examples in the body. CommonMark (and pulldown-cmark)
/// treats the inner marker as the closing fence, so the second half of
/// the outer block renders as plain text. This pass scans the source for
/// fence pairs and, when an outer fence's marker length is less than or
/// equal to the maximum fence length found inside its body, rewrites
/// both the opener and the closer to use `max_inner + 1` markers so the
/// inner ones become ordinary content.
///
/// Only fenced code blocks are affected — backticks inside inline-code
/// spans, paragraph prose, or indented code blocks are untouched
/// because the line-level fence parser ignores them.
///
/// The pass is **idempotent**: once a block's outer fence has been
/// upgraded past every inner marker, a second invocation is a no-op
/// (the rewrite condition `outer_len <= inner_max` is no longer met).
/// This matters because the streaming renderer may run the pass on
/// successive chunks of the same buffer.
fn normalize_nested_fences(markdown: &str) -> String {
    /// A fence-like line discovered during the line-classification pass.
    ///
    /// `has_info` distinguishes labeled fences (always openers) from
    /// bare fences (which may close the top of the stack).
    #[derive(Debug, Clone)]
    struct FenceLine {
        char: char,
        len: usize,
        has_info: bool,
        indent: usize,
    }

    fn parse_fence_line(line: &str) -> Option<FenceLine> {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        let indent = trimmed.chars().take_while(|c| *c == ' ').count();
        // CommonMark: > 3 leading spaces means an indented code block,
        // not a fence opener.
        if indent > 3 {
            return None;
        }
        let rest = &trimmed[indent..];
        let ch = rest.chars().next()?;
        if ch != '`' && ch != '~' {
            return None;
        }
        let len = rest.chars().take_while(|c| *c == ch).count();
        if len < 3 {
            return None;
        }
        let after = &rest[len..];
        // CommonMark: a backtick info-string may not contain a backtick.
        // Reject lines like "``` foo `bar`" so we don't misclassify
        // inline-code-bearing prose as a fence.
        if ch == '`' && after.contains('`') {
            return None;
        }
        let has_info = !after.trim().is_empty();
        Some(FenceLine {
            char: ch,
            len,
            has_info,
            indent,
        })
    }

    let lines: Vec<&str> = markdown.split_inclusive('\n').collect();
    let fence_info: Vec<Option<FenceLine>> = lines.iter().map(|l| parse_fence_line(l)).collect();

    // Pair openers with closers using a stack. Labeled fences are
    // always openers; bare fences close the top of the stack if the
    // fence char matches and the bare length is >= the opener length,
    // else they become openers themselves.
    struct StackEntry {
        line_idx: usize,
        fence: FenceLine,
    }
    let mut stack: Vec<StackEntry> = Vec::new();
    // (opener_line_idx, closer_line_idx, max_inner_fence_len)
    let mut pairs: Vec<(usize, usize, usize)> = Vec::new();

    for (i, fi) in fence_info.iter().enumerate() {
        let Some(fl) = fi else { continue };
        if fl.has_info {
            stack.push(StackEntry {
                line_idx: i,
                fence: fl.clone(),
            });
        } else {
            let closes_top = stack
                .last()
                .is_some_and(|top| top.fence.char == fl.char && fl.len >= top.fence.len);
            if closes_top {
                let opener = stack.pop().unwrap();
                let inner_max = fence_info[opener.line_idx + 1..i]
                    .iter()
                    .filter_map(|fi| fi.as_ref().map(|f| f.len))
                    .max()
                    .unwrap_or(0);
                pairs.push((opener.line_idx, i, inner_max));
            } else {
                stack.push(StackEntry {
                    line_idx: i,
                    fence: fl.clone(),
                });
            }
        }
    }

    // For each pair, rewrite opener + closer when the outer length is
    // <= the max inner length. New length is `inner_max + 1`.
    struct Rewrite {
        char: char,
        new_len: usize,
        indent: usize,
    }
    let mut rewrites: std::collections::HashMap<usize, Rewrite> = std::collections::HashMap::new();
    for (opener_idx, closer_idx, inner_max) in &pairs {
        let opener_fl = fence_info[*opener_idx].as_ref().unwrap();
        if opener_fl.len <= *inner_max {
            let new_len = inner_max + 1;
            rewrites.insert(
                *opener_idx,
                Rewrite {
                    char: opener_fl.char,
                    new_len,
                    indent: opener_fl.indent,
                },
            );
            let closer_fl = fence_info[*closer_idx].as_ref().unwrap();
            rewrites.insert(
                *closer_idx,
                Rewrite {
                    char: closer_fl.char,
                    new_len,
                    indent: closer_fl.indent,
                },
            );
        }
    }

    if rewrites.is_empty() {
        return markdown.to_string();
    }

    let mut out = String::with_capacity(markdown.len() + rewrites.len() * 4);
    for (i, line) in lines.iter().enumerate() {
        if let Some(rw) = rewrites.get(&i) {
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            let fi = fence_info[i].as_ref().unwrap();
            // Preserve the original info-string and trailing newline so
            // language labels and CRLF endings survive the rewrite.
            let info = &trimmed[fi.indent + fi.len..];
            let trailing = &line[trimmed.len()..];
            for _ in 0..rw.indent {
                out.push(' ');
            }
            for _ in 0..rw.new_len {
                out.push(rw.char);
            }
            out.push_str(info);
            out.push_str(trailing);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Block-level context the renderer is currently inside. Drives prefix +
/// flush behaviour. The `list_stack` field tracks nested lists; the top of
/// the stack is the active counter (None ⇒ bullet, Some(n) ⇒ next number).
#[derive(Debug)]
struct MarkdownRenderer<'t> {
    theme: &'t Theme,
    /// Accumulated output lines.
    out: Vec<Line<'static>>,
    /// Spans for the line currently being built.
    current: Vec<Span<'static>>,
    /// Stack of active list contexts; outermost first. None ⇒ bullet, Some(n)
    /// ⇒ ordered list with `n` as the next item number.
    list_stack: Vec<Option<u64>>,
    /// v0.9.2 W7 (SPEC §3 S8): per-level loose/tight flag, parallel to
    /// `list_stack`. CommonMark renders a *tight* list with no blank lines
    /// between items and a *loose* list with a blank line between them.
    /// pulldown-cmark 0.13's `Tag::List` does not expose the tight bit
    /// (`lib.rs:206` TODO upstream), so we infer it: a loose list wraps
    /// each item's content in a block-level `Paragraph`, whereas a tight
    /// list emits the item's inline text directly. The first
    /// `Start(Paragraph)` seen while inside a level's item flips that
    /// level to loose. We then emit the inter-item blank only for loose
    /// levels (S8 = tight-by-default, loose opt-in).
    list_loose: Vec<bool>,
    /// True while parsing inside a `BlockQuote` tag.
    in_blockquote: bool,
    /// True while parsing inside a `CodeBlock` (fenced or indented).
    in_code_block: bool,
    /// Inline style modifiers (BOLD / ITALIC) accumulated from `Strong` /
    /// `Emphasis` tags. Stored as a single mask so nested emphases compose.
    inline_modifier: Modifier,
    /// When `Some`, every inline `Text` event is treated as link anchor
    /// text and a `(url)` trailer is emitted at `End(Link)`.
    pending_link: Option<String>,
    /// URLs collected for the W3 D4 Sources block, in source order.
    link_urls: Vec<String>,
    /// True ⇒ the line we are about to flush should receive the active
    /// list prefix (bullet/number). Cleared after the first flush of an
    /// item so subsequent soft-breaks inside the item don't re-emit it.
    list_prefix_pending: bool,
    /// The prefix to emit ahead of the next list-item flush. Computed in
    /// `Start(Item)` so it reflects the current top-of-stack at that
    /// moment, even if nested items mutate the stack.
    list_prefix: Option<String>,
    /// v0.9.1.1 F6: when `Some`, every inline `Text` emitted inside the
    /// heading scope picks up this fg colour. H1/H2 set this to
    /// `theme.heading`; H3-H6 leave it `None` (plain bold only).
    /// Cleared on `End(Heading)`.
    heading_color: Option<ratatui::style::Color>,
    /// v0.9.1.2 F11: active table state. `Some` between `Tag::Table` and
    /// the matching `TagEnd::Table`. While set, inline events are
    /// captured into the current cell instead of flowing through
    /// `current` as ordinary prose.
    table: Option<TableState>,
    /// v0.9.1.2 F11-followup: terminal columns available for rendered
    /// output. When a table's natural width exceeds this budget, the
    /// `flush_table` path falls back to a bullet-list rendering instead
    /// of producing the misaligned-columns / wrapped-pipes mess Sean's
    /// screenshot showed. `u16::MAX` disables the fallback (legacy
    /// callers via [`render_markdown`]).
    content_width: u16,
}

/// State machine for a single markdown table.
///
/// All cell content is buffered into `header` / `rows` before the matching
/// `TagEnd::Table` flushes the whole grid with computed column widths.
/// This is necessary because each column's display width is the max of
/// every cell in that column, which is only known after the last row.
#[derive(Debug)]
struct TableState {
    /// Per-column alignment as reported by the parser. May be shorter than
    /// the widest row; missing entries default to `Alignment::None`.
    alignments: Vec<Alignment>,
    /// Header cells (one inner Vec per cell).
    header: Vec<Vec<Span<'static>>>,
    /// Body rows; each row is a Vec of cells; each cell is a Vec of spans.
    rows: Vec<Vec<Vec<Span<'static>>>>,
    /// Row currently being assembled (header or body).
    current_row: Vec<Vec<Span<'static>>>,
    /// Cell currently being assembled.
    current_cell: Vec<Span<'static>>,
    /// True while parsing inside `Tag::TableHead`.
    in_header: bool,
}

impl<'t> MarkdownRenderer<'t> {
    fn new(theme: &'t Theme, content_width: u16) -> Self {
        Self {
            theme,
            out: Vec::new(),
            current: Vec::new(),
            list_stack: Vec::new(),
            list_loose: Vec::new(),
            in_blockquote: false,
            in_code_block: false,
            inline_modifier: Modifier::empty(),
            pending_link: None,
            link_urls: Vec::new(),
            list_prefix_pending: false,
            list_prefix: None,
            heading_color: None,
            table: None,
            content_width,
        }
    }

    fn finalize(mut self) -> (Vec<Line<'static>>, Vec<String>) {
        // Flush any dangling spans (e.g. parser ended mid-paragraph because
        // the input is a streaming chunk).
        if !self.current.is_empty() {
            self.flush_line();
        }
        (self.out, self.link_urls)
    }

    fn handle(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(end) => self.end_tag(end),
            Event::Text(t) => self.push_text(t.into_string()),
            Event::Code(c) => self.push_inline_code(c.into_string()),
            Event::SoftBreak | Event::HardBreak => self.flush_line(),
            Event::Rule => {
                // A thematic break: render a dim horizontal line so the
                // transcript reads with visible section breaks.
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.current.push(Span::styled(
                    "─".repeat(8),
                    Style::default().fg(self.theme.text_dim),
                ));
                self.flush_line();
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                // No HTML rendering in the TUI — show the raw tag as plain
                // text so users see it didn't vanish.
                self.push_text(h.into_string());
            }
            Event::FootnoteReference(_)
            | Event::TaskListMarker(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {
                // Out of scope for v0.9.0 W2 C1 — silently drop.
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                // v0.9.2 W7 (S8): a block-level paragraph emitted while a
                // list level is active marks that list as *loose* — its
                // items get a blank line between them. (CommonMark: tight
                // lists emit item text inline with no Paragraph wrapper.)
                if let Some(loose) = self.list_loose.last_mut() {
                    *loose = true;
                }
                // Otherwise nothing to do — text/inline events will populate
                // `current` and the matching `End(Paragraph)` flushes.
            }
            Tag::Heading { level, .. } => {
                // Headings always start on a fresh line.
                if !self.current.is_empty() {
                    self.flush_line();
                }
                // v0.9.1.1 F6: drop the `#`/`##`/`###` literal prefix —
                // pulldown-cmark gives us the level out-of-band via the
                // Heading event, and rendering raw hash characters in
                // the TUI defeats the whole point of styled headers.
                // A blank spacer line precedes the heading whenever
                // there is preceding content, so headers visually
                // separate sections (mockup §5.2).
                if !self.out.is_empty() && !self.out.last().is_some_and(line_is_visually_empty) {
                    self.out.push(Line::from(Vec::<Span<'static>>::new()));
                }
                // H1/H2 get bold + heading accent color; H3 is plain
                // bold; H4-H6 still render bold (no extra color) — the
                // "1-3 styled, deeper falls through" contract.
                self.inline_modifier.insert(Modifier::BOLD);
                if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                    self.heading_color = Some(self.theme.heading);
                } else {
                    self.heading_color = None;
                }
            }
            Tag::BlockQuote(_) => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.in_blockquote = true;
            }
            Tag::CodeBlock(kind) => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.in_code_block = true;
                // v0.9.1.2 polish (Fix-A §5.2): drop the literal
                // ```lang opener — the row gets a styled language label
                // instead (`▎ lang` in text_muted on surface_hover), so
                // the block reads as a chrome-bordered card rather than
                // raw backticks. Fenced blocks with an info-string get
                // the label; bare ``` blocks and indented code blocks
                // get no label row (the left-edge `▎` bar on every body
                // line is enough signal on its own).
                if let CodeBlockKind::Fenced(lang) = kind {
                    let lang = lang.into_string();
                    if !lang.is_empty() {
                        self.emit_code_block_line(
                            Some(Span::styled(
                                lang,
                                Style::default()
                                    .fg(self.theme.text_muted)
                                    .bg(self.theme.surface_hover),
                            )),
                            true,
                        );
                    }
                }
            }
            Tag::List(start) => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.list_stack.push(start);
                // S8: assume tight until an item paragraph proves loose.
                self.list_loose.push(false);
            }
            Tag::Item => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                let indent_depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(indent_depth);
                let prefix = match self.list_stack.last_mut() {
                    Some(slot) => match slot {
                        Some(n) => {
                            let label = format!("  {indent}{n}. ");
                            *n = n.saturating_add(1);
                            label
                        }
                        None => format!("  {indent}• "),
                    },
                    None => "  • ".to_string(),
                };
                self.list_prefix = Some(prefix);
                self.list_prefix_pending = true;
            }
            Tag::Emphasis => {
                self.inline_modifier.insert(Modifier::ITALIC);
            }
            Tag::Strong => {
                self.inline_modifier.insert(Modifier::BOLD);
            }
            Tag::Link { dest_url, .. } => {
                let url = dest_url.into_string();
                self.link_urls.push(url.clone());
                self.pending_link = Some(url);
            }
            Tag::Strikethrough => {
                self.inline_modifier.insert(Modifier::CROSSED_OUT);
            }
            Tag::Table(alignments) => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.table = Some(TableState {
                    alignments,
                    header: Vec::new(),
                    rows: Vec::new(),
                    current_row: Vec::new(),
                    current_cell: Vec::new(),
                    in_header: false,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_header = true;
                    t.current_row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.current_row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.current_cell.clear();
                }
            }
            Tag::Image { .. }
            | Tag::FootnoteDefinition(_)
            | Tag::HtmlBlock
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {
                // Not in the v0.9.1.2 feature set — let inline events flow
                // through as plain text without special styling.
            }
        }
    }

    fn end_tag(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
            }
            TagEnd::Item => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                // v0.9.2 W7 (S8 variant C): tight-by-default, loose opt-in.
                // CommonMark renders a *tight* list with no blank lines
                // between items and a *loose* list with a blank line
                // between them. pulldown-cmark 0.13's public `Tag::List`
                // does NOT expose the tight bit (lib.rs:206 TODO upstream),
                // so we infer looseness in `Start(Paragraph)`: a loose
                // item wraps its content in a block-level paragraph, which
                // flips `list_loose` true for that level. We emit the
                // inter-item blank only when the innermost active list is
                // loose. (v0.9.1.3 used a `list_stack.len() <= 1` nesting
                // heuristic, which blanked every top-level tight list —
                // exactly the dense-but-misrendered case S8 corrects.)
                // Skip if the previous output line is already blank so we
                // don't stack gaps when the item ended on its own empty
                // soft-break.
                if self.list_loose.last().copied().unwrap_or(false)
                    && !self.out.last().is_some_and(line_is_visually_empty)
                {
                    self.out.push(Line::from(Vec::<Span<'static>>::new()));
                }
            }
            TagEnd::Heading(_) => {
                self.inline_modifier.remove(Modifier::BOLD);
                self.heading_color = None;
                if !self.current.is_empty() {
                    self.flush_line();
                }
            }
            TagEnd::BlockQuote(_) => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.in_blockquote = false;
            }
            TagEnd::CodeBlock => {
                // v0.9.1.2 polish (Fix-A §5.2): no literal ``` closer —
                // the left-edge `▎` bar on every body line + the
                // language-label opener already frame the block as a
                // contiguous card. Strip the trailing empty line
                // pulldown-cmark emits after the last fenced-block text
                // event so we don't leave a stray blank inside the card.
                if self.current.is_empty()
                    && self.out.last().is_some_and(|l| line_is_visually_empty(l))
                {
                    self.out.pop();
                }
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.list_stack.pop();
                self.list_loose.pop();
            }
            TagEnd::Emphasis => {
                self.inline_modifier.remove(Modifier::ITALIC);
            }
            TagEnd::Strong => {
                self.inline_modifier.remove(Modifier::BOLD);
            }
            TagEnd::Link => {
                if let Some(url) = self.pending_link.take() {
                    let trailer = Span::styled(
                        format!(" ({url})"),
                        Style::default().fg(self.theme.text_dim),
                    );
                    if let Some(t) = self.table.as_mut() {
                        t.current_cell.push(trailer);
                    } else {
                        self.current.push(trailer);
                    }
                }
            }
            TagEnd::Strikethrough => {
                self.inline_modifier.remove(Modifier::CROSSED_OUT);
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = std::mem::take(&mut t.current_cell);
                    t.current_row.push(cell);
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.current_row);
                    if t.in_header {
                        // Each row inside TableHead is a header row; the
                        // canonical grammar emits exactly one, but we
                        // tolerate the degenerate case by overwriting.
                        t.header = row;
                    } else {
                        t.rows.push(row);
                    }
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    // pulldown-cmark does NOT wrap the head row in a
                    // TableRow — it emits cells directly inside
                    // TableHead. So if we still have a non-empty
                    // current_row here, treat it as the header row.
                    if !t.current_row.is_empty() {
                        let row = std::mem::take(&mut t.current_row);
                        t.header = row;
                    }
                    t.in_header = false;
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.flush_table(t);
                }
            }
            TagEnd::Image
            | TagEnd::FootnoteDefinition
            | TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {
                // No state to unwind.
            }
        }
    }

    fn push_text(&mut self, text: String) {
        if self.in_code_block {
            // Code-block text often arrives as a single chunk containing
            // embedded `\n`s. Each \n-separated chunk becomes one
            // rendered line wearing the v0.9.1.2 polish chrome (left-edge
            // `▎` bar + surface_hover bg padded to content_width). We
            // emit those lines directly into `self.out` so they bypass
            // list/blockquote prefix logic — code blocks own their
            // entire line.
            let mut iter = text.split('\n').peekable();
            while let Some(chunk) = iter.next() {
                let body_span = if chunk.is_empty() {
                    None
                } else {
                    Some(Span::styled(
                        chunk.to_string(),
                        Style::default()
                            .fg(self.theme.orange)
                            .bg(self.theme.surface_hover),
                    ))
                };
                // A trailing empty chunk after the final `\n` is just
                // the split-iterator's tail — don't emit a phantom blank
                // row for it.
                if iter.peek().is_none() && body_span.is_none() {
                    break;
                }
                self.emit_code_block_line(body_span, false);
            }
            return;
        }
        let in_table = self.table.is_some();
        let in_header = self.table.as_ref().is_some_and(|t| t.in_header);
        let mut style = self.inline_style();
        if in_header {
            // Header cells are bold even if no Strong tag wrapped them.
            style = style.add_modifier(Modifier::BOLD);
        }
        if let Some(url) = self.pending_link.as_ref() {
            // v0.9.1.4: wrap the visible anchor text in OSC 8 hyperlink
            // escape sequences so terminals that support them (iTerm2,
            // kitty, GNOME Terminal, Apple Terminal, ghostty, Windows
            // Terminal) make the styled text Cmd-click / Ctrl-click able.
            // The escape opener/closer ride in their own raw segments so
            // ratatui's width accounting (which counts every char naively,
            // escapes included) sees only the visible text span when it
            // computes the line width — keeping wrap, table column sums,
            // and code-block padding correct. Terminals that ignore OSC 8
            // just print the visible text without any escape-induced
            // visual width.
            //
            // v0.9.2 W9 (S24): the escape pair is now emitted via the
            // shared `osc8` helpers (BEL terminator, binary-wide
            // consistent). Two guards layer on top:
            //   * mailto-strip — `mailto:` links render PLAIN (the bare
            //     anchor text in the ordinary inline style, no escape, not
            //     clickable). Users should not Cmd-click an email into a
            //     mail client by accident.
            //   * nested guard — if the anchor text already carries an
            //     OSC 8 opener (a pre-linkified payload), skip wrapping so
            //     the terminal never sees two overlapping openers.
            if osc8::is_plain_only(url) || osc8::contains_osc8(&text) {
                let span = Span::styled(text, style);
                if in_table {
                    if let Some(t) = self.table.as_mut() {
                        t.current_cell.push(span);
                    }
                } else {
                    self.current.push(span);
                }
                return;
            }
            let visible_style = style.fg(self.theme.link).add_modifier(Modifier::UNDERLINED);
            let open = Span::raw(osc8::open_seq(url));
            let visible = Span::styled(text, visible_style);
            let close = Span::raw(osc8::close_seq());
            if in_table {
                if let Some(t) = self.table.as_mut() {
                    t.current_cell.push(open);
                    t.current_cell.push(visible);
                    t.current_cell.push(close);
                }
            } else {
                self.current.push(open);
                self.current.push(visible);
                self.current.push(close);
            }
        } else {
            let span = Span::styled(text, style);
            if in_table {
                if let Some(t) = self.table.as_mut() {
                    t.current_cell.push(span);
                }
            } else {
                self.current.push(span);
            }
        }
    }

    fn push_inline_code(&mut self, code: String) {
        // Inline code is orange-on-surface_hover, regardless of
        // surrounding emphasis (it would be unreadable as italic-orange).
        // v0.9.1.2 polish (Fix-A §5.6): bumped bg from `surface` to
        // `surface_hover` so inline code stands out distinctly against
        // ordinary prose — the previous `surface` tint was too close to
        // the transcript bg to read as a chip.
        // v0.9.2 W7 (S6 variant D): VERIFIED — backticks are already
        // dropped (pulldown emits `Event::Code` content sans fences) and
        // the fg is the brand orange on the surface_hover chip. CC's S6
        // "perm-blue" maps to Wayland's brand orange (SPEC §1C chrome
        // note). No change needed; the assertion lives in the
        // `inline_code_*` tests below.
        let span = Span::styled(
            code,
            Style::default()
                .fg(self.theme.orange)
                .bg(self.theme.surface_hover),
        );
        if let Some(t) = self.table.as_mut() {
            t.current_cell.push(span);
        } else {
            self.current.push(span);
        }
    }

    /// Push one fully-styled code-block row directly into `self.out`.
    ///
    /// v0.9.1.2 polish (Fix-A §5.2): every code-block line wears the
    /// same chrome — a left-edge `▎` (U+258E) in `theme.border` so the
    /// block reads as a card with a visible side rail, plus a
    /// `theme.surface_hover` bg pad that extends to `content_width` so
    /// the tint fills the row instead of stopping at end-of-text.
    /// `body` carries the inline content (orange-on-surface_hover for
    /// real code text, text_muted-on-surface_hover for the language
    /// label). `is_label` flips the bar fg from `border` to `text_muted`
    /// so the label row reads as the card's heading, not a body row.
    ///
    /// When `content_width` is `u16::MAX` (legacy `render_markdown`
    /// callers that don't pass a viewport budget), the trailing pad is
    /// skipped — emitting ~64K spaces per line would blow the
    /// transcript out. Callers that want the polish should pass a real
    /// width via [`render_markdown_with_width`].
    fn emit_code_block_line(&mut self, body: Option<Span<'static>>, is_label: bool) {
        let bar_style = if is_label {
            Style::default()
                .fg(self.theme.text_muted)
                .bg(self.theme.surface_hover)
        } else {
            Style::default()
                .fg(self.theme.border)
                .bg(self.theme.surface_hover)
        };
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(3);
        spans.push(Span::styled("▎ ".to_string(), bar_style));
        let mut used: u16 = spans.iter().map(|s| s.width() as u16).sum();
        if let Some(b) = body {
            used = used.saturating_add(b.width() as u16);
            spans.push(b);
        }
        if self.content_width != u16::MAX && self.content_width > used {
            let pad_len = (self.content_width - used) as usize;
            spans.push(Span::styled(
                " ".repeat(pad_len),
                Style::default().bg(self.theme.surface_hover),
            ));
        }
        self.out.push(Line::from(spans));
    }

    fn inline_style(&self) -> Style {
        let mut s = Style::default().add_modifier(self.inline_modifier);
        if let Some(c) = self.heading_color {
            s = s.fg(c);
        }
        s
    }

    /// Flush `current` into `out`, applying the active block prefix.
    fn flush_line(&mut self) {
        let mut spans = std::mem::take(&mut self.current);
        if let Some(prefix) = self.list_prefix.take() {
            // Only attach the list prefix to the *first* line of an item;
            // subsequent soft-broken continuation lines just get the
            // matching indent so they align visually.
            spans.insert(
                0,
                Span::styled(prefix, Style::default().fg(self.theme.text_dim)),
            );
            self.list_prefix_pending = false;
        }
        if self.in_blockquote {
            spans.insert(
                0,
                Span::styled("│ ".to_string(), Style::default().fg(self.theme.text_dim)),
            );
        }
        self.out.push(Line::from(spans));
    }

    /// Render a completed table into `out` lines using box-drawing chars.
    ///
    /// Layout:
    ///
    /// ```text
    /// │ header1 │ header2 │ header3 │
    /// ├─────────┼─────────┼─────────┤
    /// │ row1c1  │ row1c2  │ row1c3  │
    /// │ row2c1  │ row2c2  │ row2c3  │
    /// ```
    ///
    /// * Column width = max display width across header + every body row's
    ///   cell at that column. Rows shorter than the column count are
    ///   defensively padded with empty cells.
    /// * Per-column alignment is honoured: `Center` centres the visible
    ///   text in the cell; `Right` right-pads it; `Left`/`None` left-pad.
    /// * Pipe separators (`│`) wear `theme.text_dim` so they recede; the
    ///   header / body cell text keeps its inline styling.
    ///
    /// ## v0.9.1.2 F11-followup: bullet-list fallback for wide tables
    ///
    /// When the table's natural width (sum of column widths + pipe
    /// separators + per-cell padding) exceeds the renderer's
    /// `content_width` budget, we abandon the box-drawing layout and
    /// emit a definition-list-style bullet rendering instead. Sean's
    /// screenshot showed what the old behaviour looked like when an LLM
    /// emitted a table with a 60-column URL in one cell on a 100-column
    /// terminal: misaligned columns, headers and body on different
    /// rows, raw pipes visible. The fallback turns each body row into:
    ///
    /// ```text
    /// • <first-cell>
    ///     <header2>: <cell2> · <header3>: <cell3>
    ///     <header_n>: <cell_n>
    /// ```
    ///
    /// keeping every cell visible without column alignment torture.
    fn flush_table(&mut self, mut t: TableState) {
        // Flush any dangling prose first so the table starts on its own line.
        if !self.current.is_empty() {
            self.flush_line();
        }

        let col_count = std::cmp::max(
            t.header.len(),
            t.rows.iter().map(|r| r.len()).max().unwrap_or(0),
        );
        if col_count == 0 {
            return;
        }

        // Defensive pad: every row (header + body) gets at least
        // `col_count` cells. Missing cells are empty.
        let pad_row = |row: &mut Vec<Vec<Span<'static>>>| {
            while row.len() < col_count {
                row.push(Vec::new());
            }
        };
        pad_row(&mut t.header);
        for r in t.rows.iter_mut() {
            pad_row(r);
        }

        // Compute display width per column. We use Span content lengths
        // (counted as chars, not bytes) — every cell is plain ASCII or
        // a multi-byte UTF-8 char which still occupies one column in the
        // terminal for the common cases we render (single-line theme).
        let cell_width = |cell: &Vec<Span<'static>>| -> usize {
            cell.iter().map(|s| s.content.chars().count()).sum()
        };
        let mut col_widths = vec![0usize; col_count];
        for (i, cell) in t.header.iter().enumerate() {
            col_widths[i] = col_widths[i].max(cell_width(cell));
        }
        for row in &t.rows {
            for (i, cell) in row.iter().enumerate() {
                col_widths[i] = col_widths[i].max(cell_width(cell));
            }
        }
        // Minimum visible width so a fully-empty column still draws a
        // pipe-separator with a single space of padding (`│ │`).
        for w in col_widths.iter_mut() {
            if *w == 0 {
                *w = 1;
            }
        }

        // v0.9.1.2 F11-followup: width gate. The box-drawing layout
        // emits `│ cell │ cell │ … │` so the natural width is the sum
        // of column widths + `2 * col_count` (one space pad each side
        // of every cell) + `col_count + 1` pipes. If that exceeds the
        // renderer's `content_width` budget we bail to the bullet-list
        // fallback. `u16::MAX` ⇒ never trigger (legacy callers).
        let natural_width: usize =
            col_widths.iter().sum::<usize>() + 2 * col_count + (col_count + 1);
        if natural_width > self.content_width as usize {
            self.flush_table_as_bullet_list(t);
            return;
        }

        let dim = Style::default().fg(self.theme.text_dim);
        let pipe = || Span::styled("│".to_string(), dim);
        let space = |n: usize| Span::raw(" ".repeat(n));

        // Resolve per-column alignment with a safe default for any
        // column past the parser-reported alignment vector.
        let align_for =
            |idx: usize| -> Alignment { t.alignments.get(idx).copied().unwrap_or(Alignment::None) };

        // Build a single rendered cell line: " <padded spans> ".
        let render_cell =
            |cell: Vec<Span<'static>>, width: usize, align: Alignment| -> Vec<Span<'static>> {
                let content_width = cell
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum::<usize>();
                let slack = width.saturating_sub(content_width);
                let (lpad, rpad) = match align {
                    Alignment::Center => {
                        let l = slack / 2;
                        (l, slack - l)
                    }
                    Alignment::Right => (slack, 0),
                    Alignment::Left | Alignment::None => (0, slack),
                };
                let mut out = Vec::with_capacity(cell.len() + 4);
                out.push(space(1)); // leading inner padding
                if lpad > 0 {
                    out.push(space(lpad));
                }
                out.extend(cell);
                if rpad > 0 {
                    out.push(space(rpad));
                }
                out.push(space(1)); // trailing inner padding
                out
            };

        // Helper to build a complete row line (header or body).
        let build_row = |cells: Vec<Vec<Span<'static>>>| -> Line<'static> {
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(pipe());
            for (i, cell) in cells.into_iter().enumerate() {
                spans.extend(render_cell(cell, col_widths[i], align_for(i)));
                spans.push(pipe());
            }
            Line::from(spans)
        };

        // 1) Header row.
        let header_cells = std::mem::take(&mut t.header);
        self.out.push(build_row(header_cells));

        // 2) Separator row: ├───┼───┼───┤
        let mut sep: Vec<Span<'static>> = Vec::new();
        sep.push(Span::styled("├".to_string(), dim));
        for (i, w) in col_widths.iter().enumerate() {
            // +2 for the inner space pad on each side of the cell.
            sep.push(Span::styled("─".repeat(w + 2), dim));
            let last = i + 1 == col_widths.len();
            sep.push(Span::styled(if last { "┤" } else { "┼" }.to_string(), dim));
        }
        self.out.push(Line::from(sep));

        // 3) Body rows.
        for row in t.rows.drain(..) {
            self.out.push(build_row(row));
        }
    }

    /// Wide-table fallback: render the table as a bullet list with each
    /// body row's first cell as the bullet title and the remaining cells
    /// as `<header>: <cell>` pairs joined by ` · ` on continuation lines.
    ///
    /// Cells are emitted preserving their existing inline styling (so a
    /// link cell still shows the URL trailer, an inline-code cell stays
    /// orange-on-surface, etc). Empty rows are skipped. A header-less
    /// table (none of the body rows had a `<thead>`) falls back to
    /// joining every cell with ` · ` on the bullet line, no key prefix —
    /// the best we can do without column labels.
    ///
    /// Per Sean's contract: "If it's too long, we don't do a table. We
    /// turn it into a bullet point list or something like that."
    fn flush_table_as_bullet_list(&mut self, mut t: TableState) {
        // Flatten each header cell into a single plain string for the
        // `<header>: <cell>` prefixes. We deliberately drop styling on
        // headers in the fallback so the prefix reads as a label rather
        // than a heavyweight bold token — this matches the spec mockup.
        let header_labels: Vec<String> = t
            .header
            .iter()
            .map(|cell| cell.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        let has_header =
            !header_labels.is_empty() && header_labels.iter().any(|h| !h.trim().is_empty());

        let dim = Style::default().fg(self.theme.text_dim);
        let middot = || Span::styled(" · ".to_string(), dim);

        for row in t.rows.drain(..) {
            // Skip rows that are entirely empty.
            let all_empty = row
                .iter()
                .all(|cell| cell.iter().all(|s| s.content.trim().is_empty()));
            if all_empty {
                continue;
            }

            // Bullet line: `• <first-cell-spans>`.
            let mut iter = row.into_iter();
            let first = iter.next().unwrap_or_default();
            let mut line_spans: Vec<Span<'static>> = Vec::with_capacity(first.len() + 1);
            line_spans.push(Span::styled("• ".to_string(), dim));
            line_spans.extend(first);
            self.out.push(Line::from(line_spans));

            // Continuation line(s): `    <h1>: <cell1> · <h2>: <cell2>`.
            // We emit ALL remaining cells on a single indented line
            // joined by ` · ` so the row reads as one logical record,
            // matching the briefing's acceptance example. Cells whose
            // content is entirely whitespace are skipped (no
            // `<header>: ` orphan for an empty cell).
            let rest: Vec<Vec<Span<'static>>> = iter.collect();
            let mut continuation: Vec<Span<'static>> = Vec::new();
            let mut wrote_any = false;
            for (idx, cell) in rest.into_iter().enumerate() {
                let cell_blank = cell.iter().all(|s| s.content.trim().is_empty());
                if cell_blank {
                    continue;
                }
                if wrote_any {
                    continuation.push(middot());
                }
                if has_header {
                    // `idx` is the offset into `rest`, which corresponds
                    // to column `idx + 1` in the original row.
                    let label = header_labels.get(idx + 1).map(|s| s.trim()).unwrap_or("");
                    if !label.is_empty() {
                        continuation.push(Span::styled(format!("{label}: "), dim));
                    }
                }
                continuation.extend(cell);
                wrote_any = true;
            }
            if wrote_any {
                let mut spans: Vec<Span<'static>> = Vec::with_capacity(continuation.len() + 1);
                spans.push(Span::raw("    "));
                spans.extend(continuation);
                self.out.push(Line::from(spans));
            }
        }
    }
}

fn line_is_visually_empty(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .all(|s| s.content.chars().all(char::is_whitespace))
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn theme() -> Theme {
        Theme::hearth()
    }

    /// Concatenate every span's text on a line — the displayed string.
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Current per-thread memo size (test-only probe). Used to assert hits vs
    /// misses without exposing the cache. The thread-local persists across
    /// tests on the same worker, so the memo tests use unique source strings
    /// and assert on the *delta*, never an absolute count.
    fn cache_len() -> usize {
        MARKDOWN_CACHE.with(|c| c.borrow().map.len())
    }

    #[test]
    fn second_identical_render_hits_the_cache() {
        let th = theme();
        // Unique source → its key cannot pre-exist from another test.
        let src = "perf-probe-§hit **bold** plus `code` and a [link](https://x.y)";
        let before = cache_len();
        let first = render_markdown_with_width(src, &th, 80);
        let after_first = cache_len();
        let second = render_markdown_with_width(src, &th, 80);
        let after_second = cache_len();

        assert_eq!(
            after_first,
            before + 1,
            "first render must insert one entry"
        );
        assert_eq!(
            after_second, after_first,
            "second identical render must hit the cache, not grow it"
        );
        // Cached output is byte-for-byte the fresh output (no corruption).
        assert_eq!(first.0.len(), second.0.len());
        for (a, b) in first.0.iter().zip(second.0.iter()) {
            assert_eq!(line_text(a), line_text(b));
        }
        assert_eq!(first.1, second.1, "collected URLs must match");
    }

    #[test]
    fn theme_change_keys_a_fresh_entry_no_stale_colors() {
        let src = "perf-probe-§theme # Heading text";
        let _ = render_markdown_with_width(src, &Theme::hearth(), 80);
        let len_after_first = cache_len();
        // Same source + width but a different heading color MUST produce a new
        // key — otherwise a theme switch would surface stale, wrong-colored
        // lines from the cache.
        let mut alt = Theme::hearth();
        alt.heading = ratatui::style::Color::Rgb(1, 2, 3);
        let _ = render_markdown_with_width(src, &alt, 80);
        assert_eq!(
            cache_len(),
            len_after_first + 1,
            "a theme change must miss the cache and render fresh"
        );
    }

    #[test]
    fn width_change_keys_a_fresh_entry() {
        let th = theme();
        let src = "perf-probe-§width plain paragraph text that wraps differently";
        let _ = render_markdown_with_width(src, &th, 80);
        let len_after_first = cache_len();
        let _ = render_markdown_with_width(src, &th, 40);
        assert_eq!(
            cache_len(),
            len_after_first + 1,
            "a width change must miss the cache (wrapping differs)"
        );
    }

    #[test]
    fn cached_output_equals_uncached_output() {
        // The cached wrapper must return exactly what the raw renderer does.
        let th = theme();
        let src = "perf-probe-§equiv\n\n- one\n- two\n\n```rust\nlet x = 1;\n```\n";
        let cached = render_markdown_with_width(src, &th, 72);
        let raw = render_markdown_with_width_uncached(src, &th, 72);
        assert_eq!(cached.0.len(), raw.0.len());
        for (a, b) in cached.0.iter().zip(raw.0.iter()) {
            assert_eq!(line_text(a), line_text(b));
        }
        assert_eq!(cached.1, raw.1);
    }

    #[test]
    fn bold_text_carries_bold_modifier() {
        let (lines, urls) = render_markdown("**hi**", &theme());
        assert!(urls.is_empty(), "no links should be extracted");
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        // The single Text event sits inside Strong, so the span carries BOLD.
        let bold_span = line
            .spans
            .iter()
            .find(|s| s.content == "hi")
            .expect("bold span present");
        assert!(
            bold_span.style.add_modifier.contains(Modifier::BOLD),
            "bold span must carry Modifier::BOLD; got {:?}",
            bold_span.style
        );
    }

    #[test]
    fn italic_text_carries_italic_modifier() {
        let (lines, _) = render_markdown("*hi*", &theme());
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "hi")
            .expect("italic span");
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn inline_code_uses_orange_on_surface() {
        // v0.9.1.2 polish: inline code now uses `surface_hover` bg (not
        // `surface`) so it stands out distinctly against ordinary prose.
        // The orange fg is unchanged.
        let t = theme();
        let (lines, _) = render_markdown("text `code` after", &t);
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "code")
            .expect("code span");
        assert_eq!(span.style.fg, Some(t.orange));
        assert_eq!(span.style.bg, Some(t.surface_hover));
    }

    // ── v0.9.1.1 F6 — styled markdown headers (no literal `#` chars) ──

    /// Concatenate every span's text across every line into one string —
    /// used to scan the entire rendered output for literal hash chars.
    fn all_text(lines: &[Line<'_>]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn render_markdown_level_1_header_renders_bold_accent() {
        let t = theme();
        let (lines, _) = render_markdown("# Hello", &t);
        // Heading body line — find the "Hello" span.
        let body = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "Hello")
            .expect("heading body span");
        assert!(body.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(body.style.fg, Some(t.heading));
        // No `#` char survives anywhere in the output.
        assert!(
            !all_text(&lines).contains('#'),
            "literal `#` leaked into H1 output:\n{}",
            all_text(&lines)
        );
    }

    #[test]
    fn render_markdown_level_2_header_renders_bold_accent() {
        let t = theme();
        let (lines, _) = render_markdown("## Section", &t);
        let body = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "Section")
            .expect("heading body span");
        assert!(body.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(body.style.fg, Some(t.heading));
        assert!(
            !all_text(&lines).contains('#'),
            "literal `#` leaked into H2 output:\n{}",
            all_text(&lines)
        );
    }

    #[test]
    fn render_markdown_level_3_header_renders_bold() {
        let t = theme();
        let (lines, _) = render_markdown("### Subhead", &t);
        let body = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "Subhead")
            .expect("heading body span");
        // H3 is bold but does NOT take the heading accent color.
        assert!(body.style.add_modifier.contains(Modifier::BOLD));
        assert_ne!(
            body.style.fg,
            Some(t.heading),
            "H3 should be plain bold, not accent-coloured"
        );
        assert!(
            !all_text(&lines).contains('#'),
            "literal `#` leaked into H3 output:\n{}",
            all_text(&lines)
        );
    }

    #[test]
    fn render_markdown_h2_followed_by_paragraph_renders_separated() {
        // `## Title\n\nbody` should emit: optional leading blank, the
        // heading line, then a blank spacer, then the body — verifying
        // headers visually break the flow even when they're the first
        // block (no leading blank required there).
        let (lines, _) = render_markdown("intro\n\n## Title\n\nbody\n", &theme());
        let text_lines: Vec<String> = lines.iter().map(line_text).collect();
        // Find the heading and body line indices.
        let h_idx = text_lines
            .iter()
            .position(|l| l.contains("Title"))
            .expect("heading line");
        let b_idx = text_lines
            .iter()
            .position(|l| l == "body")
            .expect("body line");
        assert!(
            h_idx > 0,
            "heading must not be the first line (intro precedes)"
        );
        assert!(
            text_lines[h_idx - 1].trim().is_empty(),
            "missing blank spacer before heading: {:?}",
            text_lines
        );
        assert!(b_idx > h_idx, "body line must come after heading");
    }

    #[test]
    fn render_markdown_headers_no_literal_hash_chars_v0911() {
        // Comprehensive: every heading level 1-3, mixed with bold and a
        // bullet list, must produce zero literal `#` characters in the
        // rendered output bytes. This is the regression guard for
        // v0.9.1.1 F6 ("# and ## chars showing in long replies").
        let src = "# Top\n\nplain text\n\n## Mid **bold**\n\n- item\n\n### Sub\n\nmore text\n";
        let (lines, _) = render_markdown(src, &theme());
        let text = all_text(&lines);
        assert!(
            !text.contains('#'),
            "literal `#` chars survived markdown header rendering:\n{text}"
        );
        // Sanity: the actual heading words made it through.
        assert!(text.contains("Top"), "H1 body missing:\n{text}");
        assert!(text.contains("Mid"), "H2 body missing:\n{text}");
        assert!(text.contains("Sub"), "H3 body missing:\n{text}");
    }

    #[test]
    fn bullet_list_emits_bullet_prefix() {
        // v0.9.1.3 polish: top-level items now get a blank Line after
        // each item end (legibility gap). Filter visually-empty lines
        // before asserting prefixes so this test stays focused on
        // bullet rendering, not spacing — that's covered by the
        // dedicated `_v0913` tests below.
        let src = "- one\n- two\n";
        let (lines, _) = render_markdown(src, &theme());
        let bullets: Vec<&Line<'_>> = lines
            .iter()
            .filter(|l| !line_is_visually_empty(l))
            .collect();
        assert_eq!(bullets.len(), 2);
        let first = line_text(bullets[0]);
        assert!(
            first.starts_with("  • "),
            "expected bullet prefix, got {first:?}"
        );
        assert!(first.contains("one"));
        assert!(line_text(bullets[1]).starts_with("  • "));
    }

    #[test]
    fn numbered_list_advances_counter() {
        // v0.9.1.3 polish: filter blank-line gaps before asserting
        // counter prefixes (gap contract owned by `_v0913` tests).
        let src = "1. alpha\n2. beta\n3. gamma\n";
        let (lines, _) = render_markdown(src, &theme());
        let items: Vec<&Line<'_>> = lines
            .iter()
            .filter(|l| !line_is_visually_empty(l))
            .collect();
        assert_eq!(items.len(), 3);
        assert!(line_text(items[0]).starts_with("  1. "));
        assert!(line_text(items[1]).starts_with("  2. "));
        assert!(line_text(items[2]).starts_with("  3. "));
    }

    #[test]
    fn numbered_list_honors_explicit_start() {
        // v0.9.1.3 polish: filter blank-line gaps before asserting prefixes.
        let src = "5. alpha\n6. beta\n";
        let (lines, _) = render_markdown(src, &theme());
        let items: Vec<&Line<'_>> = lines
            .iter()
            .filter(|l| !line_is_visually_empty(l))
            .collect();
        assert!(line_text(items[0]).starts_with("  5. "));
        assert!(line_text(items[1]).starts_with("  6. "));
    }

    #[test]
    fn code_fence_emits_language_label_dim_on_opener() {
        // v0.9.1.2 polish: the literal "```lang" opener was replaced
        // with a styled language label row — the lang text wears
        // `text_muted` on `surface_hover` so it reads as a card heading
        // rather than raw backticks. Body lines wear `▎ ` (border on
        // surface_hover) followed by the code text (orange on
        // surface_hover). No closing "```" row.
        let t = theme();
        let src = "```rust\nfn main() {}\n```";
        let (lines, _) = render_markdown(src, &t);
        // Expect: label "rust", body "fn main() {}". No literal fence rows.
        assert!(lines.len() >= 2, "got lines: {lines:?}");
        let label = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "rust")
            .expect("language label span");
        assert_eq!(label.style.fg, Some(t.text_muted));
        assert_eq!(label.style.bg, Some(t.surface_hover));
        let body = lines[1]
            .spans
            .iter()
            .find(|s| s.content == "fn main() {}")
            .expect("body span");
        assert_eq!(body.style.fg, Some(t.orange));
        assert_eq!(body.style.bg, Some(t.surface_hover));
        // No span anywhere in the output is the literal "```" fence.
        let all = all_text(&lines);
        assert!(
            !all.contains("```"),
            "literal ``` leaked into code-block output:\n{all}"
        );
    }

    #[test]
    fn code_fence_without_language_emits_code_label() {
        // v0.9.1.2 polish: bare ``` blocks no longer emit a synthetic
        // "code" label — the left-edge `▎` bar on every body line is
        // signal enough on its own. The first line is therefore the
        // body row, not a label row.
        let t = theme();
        let src = "```\nhello\n```";
        let (lines, _) = render_markdown(src, &t);
        // No literal "```code" or "```" anywhere.
        let all = all_text(&lines);
        assert!(
            !all.contains("```"),
            "literal ``` leaked into bare code-block output:\n{all}"
        );
        // The body line wears the polish chrome.
        let body = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "hello")
            .expect("body span");
        assert_eq!(body.style.fg, Some(t.orange));
        assert_eq!(body.style.bg, Some(t.surface_hover));
    }

    // ── v0.9.1.3 polish wave 1A — code-block + inline-code styling ──

    #[test]
    fn code_block_renders_with_surface_bg_v0913() {
        // Every span of every code-block body line must paint
        // `surface_hover` as its background — that's the tint that turns
        // the block into a visible card against the transcript bg. We
        // check across both the `▎ ` bar span and the body text span.
        let t = theme();
        let src = "```rust\nfn main() {}\n```";
        let (lines, _) = render_markdown(src, &t);
        // Locate the body row (the one carrying the `fn main() {}` text).
        let body_row = lines
            .iter()
            .find(|l| line_text(l).contains("fn main() {}"))
            .expect("body row");
        let body_span = body_row
            .spans
            .iter()
            .find(|s| s.content == "fn main() {}")
            .expect("body span");
        assert_eq!(
            body_span.style.bg,
            Some(t.surface_hover),
            "code-block body span must paint surface_hover bg",
        );
        // The leading `▎ ` bar span also wears the surface_hover bg so
        // the card reads as a contiguous tinted region with the bar on
        // its left edge.
        let bar_span = &body_row.spans[0];
        assert!(
            bar_span.content.starts_with('▎'),
            "code-block body row must lead with `▎` bar; got {:?}",
            bar_span.content
        );
        assert_eq!(
            bar_span.style.bg,
            Some(t.surface_hover),
            "code-block bar span must paint surface_hover bg",
        );
    }

    #[test]
    fn code_block_language_label_renders_v0913() {
        // The first emitted line of a fenced code block with a non-empty
        // info-string must surface the language token in `text_muted` so
        // the row reads as a card heading rather than a body row.
        let t = theme();
        let src = "```rust\nfn main() {}\n```";
        let (lines, _) = render_markdown(src, &t);
        let first = &lines[0];
        let first_text = line_text(first);
        assert!(
            first_text.contains("rust"),
            "first emitted code-block line must contain language label `rust`; got {first_text:?}"
        );
        let label = first
            .spans
            .iter()
            .find(|s| s.content == "rust")
            .expect("language-label span");
        assert_eq!(
            label.style.fg,
            Some(t.text_muted),
            "language label must wear text_muted fg",
        );
        assert_eq!(
            label.style.bg,
            Some(t.surface_hover),
            "language label must wear surface_hover bg",
        );
    }

    #[test]
    fn inline_code_distinct_style_v0913() {
        // Inline `code` must paint a bg or fg distinct from every
        // surrounding prose span in the same line — otherwise it reads
        // as plain text and the recon §5.6 polish bar fails.
        let t = theme();
        let (lines, _) = render_markdown("Use `foo` here", &t);
        let line = &lines[0];
        let code = line
            .spans
            .iter()
            .find(|s| s.content == "foo")
            .expect("inline-code span");
        // Find at least one prose span (non-code text content) for the
        // distinctness comparison.
        let prose = line
            .spans
            .iter()
            .find(|s| s.content != "foo" && !s.content.trim().is_empty())
            .expect("at least one prose span");
        assert!(
            code.style.fg != prose.style.fg || code.style.bg != prose.style.bg,
            "inline code must be visually distinct from prose: code={:?} prose={:?}",
            code.style,
            prose.style
        );
        // And specifically, the v0.9.1.2 polish chip wears surface_hover.
        assert_eq!(code.style.bg, Some(t.surface_hover));
    }

    #[test]
    fn link_appends_url_and_collects_into_returned_vec() {
        let t = theme();
        let src = "see [Rust](https://rust-lang.org) please";
        let (lines, urls) = render_markdown(src, &t);
        assert_eq!(urls, vec!["https://rust-lang.org".to_string()]);
        // Anchor text in link color.
        let anchor = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "Rust")
            .expect("link anchor span");
        assert_eq!(anchor.style.fg, Some(t.link));
        // URL trailer present and dim. v0.9.1.4: the OSC 8 opener span
        // ALSO contains the URL substring (`\x1b]8;;<url>\x07`), so filter
        // it out before looking for the trailer.
        let trailer = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("https://rust-lang.org") && !s.content.starts_with('\x1b'))
            .expect("url trailer span");
        assert_eq!(trailer.style.fg, Some(t.text_dim));
        assert_eq!(trailer.content.as_ref(), " (https://rust-lang.org)");
    }

    #[test]
    fn multiple_links_collected_in_source_order() {
        let src = "[a](http://a.test) and [b](http://b.test)";
        let (_, urls) = render_markdown(src, &theme());
        assert_eq!(urls, vec!["http://a.test", "http://b.test"]);
    }

    // ── v0.9.1.4 — OSC 8 clickable hyperlinks ──
    //
    // Wraps the visible link anchor text in `\x1b]8;;<url>\x07 … \x1b]8;;\x07`
    // so terminals that honor OSC 8 (iTerm2, kitty, GNOME Terminal, Apple
    // Terminal, ghostty, Windows Terminal) make the styled text
    // Cmd-click / Ctrl-click able. Terminals that ignore OSC 8 still
    // render the visible text with zero visual width cost from the
    // escape sequences. The opener / closer ride in their own
    // `Span::raw` segments so ratatui's width accounting (which counts
    // chars naively) only sees the visible text span.

    #[test]
    fn markdown_link_emits_osc8_escape_sequence_v0914() {
        let (lines, _) = render_markdown("See [docs](https://example.com).", &theme());
        let line = &lines[0];
        let has_open = line
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;https://example.com\x07");
        let has_close = line
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;\x07");
        assert!(
            has_open,
            "expected OSC 8 opener span `\\x1b]8;;https://example.com\\x07`; got spans: {:?}",
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
        );
        assert!(
            has_close,
            "expected OSC 8 closer span `\\x1b]8;;\\x07`; got spans: {:?}",
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn markdown_link_visible_text_styled_v0914() {
        // The visible "docs" anchor must keep its link-color fg AND wear
        // Modifier::UNDERLINED so it's distinguishable as a clickable
        // hyperlink even on terminals that ignore the OSC 8 escapes.
        let t = theme();
        let (lines, _) = render_markdown("See [docs](https://example.com).", &t);
        let anchor = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "docs")
            .expect("visible link anchor span 'docs'");
        assert_eq!(
            anchor.style.fg,
            Some(t.link),
            "link anchor missing link color"
        );
        assert!(
            anchor.style.add_modifier.contains(Modifier::UNDERLINED),
            "link anchor missing UNDERLINED modifier; got style: {:?}",
            anchor.style
        );
    }

    #[test]
    fn markdown_auto_link_clickable_v0914() {
        // Pulldown-cmark with `Options::ENABLE_TABLES |
        // ENABLE_STRIKETHROUGH` (our prod options) does not auto-detect
        // bare URLs — those need `<https://example.com>` angle-bracket
        // autolink syntax to become `Tag::Link` events. That's still a
        // valid CommonMark link form (`<url>`) and the renderer must
        // wrap it in OSC 8 just like an inline `[text](url)`.
        let (lines, urls) = render_markdown("Visit <https://example.com> today", &theme());
        assert_eq!(urls, vec!["https://example.com".to_string()]);
        let line = &lines[0];
        let has_open = line
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;https://example.com\x07");
        let has_close = line
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;\x07");
        assert!(has_open, "auto-link missing OSC 8 opener");
        assert!(has_close, "auto-link missing OSC 8 closer");
    }

    #[test]
    fn markdown_link_in_table_cell_clickable_v0914() {
        // A markdown table cell containing a link must still emit OSC 8
        // wrapping around the visible anchor — the table flush path
        // routes cell spans through a different code path than ordinary
        // prose, so this guards against regressions where table cells
        // drop the escape spans during flush.
        let src = "| name | n |\n|------|---|\n| [foo](https://x.test) | 1 |\n";
        let (lines, _) = render_markdown(src, &theme());
        let body = lines.last().expect("body row");
        let has_open = body
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;https://x.test\x07");
        let has_close = body
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;\x07");
        assert!(
            has_open,
            "table cell link missing OSC 8 opener; body spans: {:?}",
            body.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
        );
        assert!(
            has_close,
            "table cell link missing OSC 8 closer; body spans: {:?}",
            body.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn markdown_mailto_link_renders_plain_no_osc8_v092() {
        // v0.9.2 W9 (S24 mailto-strip): a `[mail](mailto:a@b.com)` link
        // renders the anchor text PLAIN — no OSC 8 escape spans at all.
        let (lines, _) = render_markdown("Email [me](mailto:a@b.com) now.", &theme());
        let line = &lines[0];
        let any_osc8 = line.spans.iter().any(|s| s.content.contains("\x1b]8;;"));
        assert!(
            !any_osc8,
            "mailto link must not emit any OSC 8 escape; got spans: {:?}",
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
        );
        // The visible anchor text is still present, just plain.
        let has_anchor = line.spans.iter().any(|s| s.content.as_ref() == "me");
        assert!(has_anchor, "mailto anchor text 'me' must still render");
    }

    #[test]
    fn markdown_mailto_anchor_not_link_styled_v092() {
        // The plain mailto anchor must NOT wear the link fg / underline —
        // it reads as ordinary prose, signalling it is not clickable.
        let t = theme();
        let (lines, _) = render_markdown("Write [team](mailto:t@x.com).", &t);
        let anchor = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "team")
            .expect("mailto anchor span 'team'");
        assert!(
            !anchor.style.add_modifier.contains(Modifier::UNDERLINED),
            "mailto anchor must not be underlined; style: {:?}",
            anchor.style
        );
    }

    #[test]
    fn markdown_http_link_still_emits_osc8_after_w9_v092() {
        // Regression guard: routing through `osc8::open_seq`/`close_seq`
        // must produce byte-identical escapes to the v0.9.1.4 inline form
        // so a real http link is still clickable.
        let (lines, _) = render_markdown("See [docs](https://example.com).", &theme());
        let line = &lines[0];
        assert!(
            line.spans
                .iter()
                .any(|s| s.content.as_ref() == "\x1b]8;;https://example.com\x07"),
            "http link must still emit the OSC 8 opener via osc8 helper"
        );
        assert!(
            line.spans
                .iter()
                .any(|s| s.content.as_ref() == "\x1b]8;;\x07"),
            "http link must still emit the OSC 8 closer via osc8 helper"
        );
    }

    #[test]
    fn blockquote_emits_pipe_prefix_dim() {
        let t = theme();
        let src = "> quote me\n";
        let (lines, _) = render_markdown(src, &t);
        assert_eq!(lines.len(), 1);
        let prefix = &lines[0].spans[0];
        assert_eq!(prefix.content.as_ref(), "│ ");
        assert_eq!(prefix.style.fg, Some(t.text_dim));
        assert!(line_text(&lines[0]).contains("quote me"));
    }

    #[test]
    fn empty_input_produces_no_lines_and_no_urls() {
        let (lines, urls) = render_markdown("", &theme());
        assert!(lines.is_empty());
        assert!(urls.is_empty());
    }

    #[test]
    fn partial_chunk_mid_code_fence_does_not_panic() {
        // Streaming-safe: input ends mid-fence. C5 owns split-point logic;
        // C1's contract is "render whatever the parser yields, no panic."
        let src = "```rust\nfn foo() {\n    let x = ";
        let (lines, _) = render_markdown(src, &theme());
        // No assertions about line count — just that we got *something*
        // back and didn't crash.
        assert!(!lines.is_empty());
    }

    #[test]
    fn softbreak_within_paragraph_splits_into_two_lines() {
        // pulldown-cmark emits SoftBreak for a single `\n` inside a para.
        let src = "first\nsecond\n";
        let (lines, _) = render_markdown(src, &theme());
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "first");
        assert_eq!(line_text(&lines[1]), "second");
    }

    #[test]
    fn hardbreak_within_paragraph_splits_into_two_lines() {
        // Two trailing spaces => HardBreak.
        let src = "first  \nsecond\n";
        let (lines, _) = render_markdown(src, &theme());
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn nested_emphasis_composes_modifiers() {
        let src = "***both***";
        let (lines, _) = render_markdown(src, &theme());
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "both")
            .expect("nested emphasis text");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }

    // ── normalize_nested_fences ──────────────────────────────────────

    #[test]
    fn normalize_passes_through_text_with_no_fences() {
        let src = "just some prose with `inline code` and no fences\nsecond line\n";
        assert_eq!(normalize_nested_fences(src), src);
    }

    #[test]
    fn normalize_passes_through_simple_single_fence() {
        // Outer fence of 3, body contains no backtick run >= 3 → no rewrite.
        let src = "```rust\nfn main() {}\n```\n";
        assert_eq!(normalize_nested_fences(src), src);
    }

    #[test]
    fn normalize_upgrades_outer_to_4_when_body_has_3() {
        // Markdown about markdown: the outer block carries info-string
        // "markdown" and demonstrates a nested fenced block whose own
        // opener is labeled "rust". Without normalization the inner
        // bare closing fence terminates the outer block early.
        let src = "```markdown\n```rust\nfn x() {}\n```\nDone.\n```\n";
        let out = normalize_nested_fences(src);
        // The labeled opener and the final bare closer should both
        // grow to 4 backticks while the inner 3-backtick markers stay.
        assert!(
            out.starts_with("````markdown\n"),
            "expected upgraded opener, got: {out:?}"
        );
        assert!(
            out.ends_with("````\n"),
            "expected upgraded closer, got: {out:?}"
        );
        // Inner 3-backtick markers must be preserved verbatim.
        assert!(out.contains("\n```rust\nfn x() {}\n```\n"));
    }

    #[test]
    fn normalize_handles_tilde_fences() {
        // Outer tilde fence with an inner labeled tilde fence — should
        // upgrade both opener + closer to 4 tildes.
        let src = "~~~markdown\n~~~rust\nfn x() {}\n~~~\n~~~\n";
        let out = normalize_nested_fences(src);
        assert!(out.starts_with("~~~~markdown\n"));
        assert!(out.ends_with("~~~~\n"));
        assert!(out.contains("\n~~~rust\nfn x() {}\n~~~\n"));
    }

    #[test]
    fn normalize_idempotent() {
        // After the first upgrade, the outer fence is longer than any
        // inner run, so a second pass must produce the same string.
        let src = "```markdown\n```rust\nfoo\n```\n```\n";
        let once = normalize_nested_fences(src);
        let twice = normalize_nested_fences(&once);
        assert_eq!(once, twice, "normalize_nested_fences must be idempotent");
    }

    #[test]
    fn render_markdown_renders_nested_fence_correctly() {
        // The bug scenario from the briefing: an LLM showing a markdown
        // example that itself contains a fenced code block. Before the
        // pre-pass, "Done." rendered outside any code block (as plain
        // prose); after the pass it must stay inside the outer block.
        //
        // v0.9.1.2 polish: the outer block now renders as a styled
        // card — first row is the `markdown` language label
        // (text_muted on surface_hover), every following row is a
        // body line wearing `▎ ` + code text. No literal "```" fences
        // are emitted by the renderer; inner backticks survive as
        // body content because they're text events inside the outer
        // CodeBlock.
        let t = theme();
        let src = "```markdown\n```rust\nfn x() {}\n```\nDone.\n```\n";
        let (lines, _) = render_markdown(src, &t);

        // First row: language label "markdown" in text_muted.
        let label = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "markdown")
            .expect("language label span");
        assert_eq!(label.style.fg, Some(t.text_muted));
        assert_eq!(label.style.bg, Some(t.surface_hover));

        // Inner content must survive as code-styled body lines.
        let body_text = all_text(&lines[1..]);
        assert!(
            body_text.contains("```rust"),
            "inner labeled fence should render as content, got {body_text:?}"
        );
        assert!(
            body_text.contains("fn x() {}"),
            "nested example body should be inside outer block, got {body_text:?}"
        );
        assert!(
            body_text.contains("Done."),
            "trailing line must stay inside the outer block, got {body_text:?}"
        );
        // Every non-whitespace, non-bar body span carries the code
        // chrome (orange fg, surface_hover bg). The `▎ ` bar carries
        // border fg and is checked separately.
        for line in &lines[1..] {
            for span in &line.spans {
                if span.content.starts_with('▎') {
                    continue;
                }
                if span.content.chars().any(|c| !c.is_whitespace()) {
                    assert_eq!(
                        span.style.fg,
                        Some(t.orange),
                        "body span {:?} should carry code fg",
                        span.content
                    );
                    assert_eq!(
                        span.style.bg,
                        Some(t.surface_hover),
                        "body span {:?} should carry code bg",
                        span.content
                    );
                }
            }
        }

        // No literal triple-backtick row should appear in the output.
        let all = all_text(&lines);
        let row_count = lines
            .iter()
            .filter(|l| line_text(l).trim() == "```")
            .count();
        assert_eq!(
            row_count, 0,
            "renderer must not emit a literal ``` closer row; got all output:\n{all}"
        );
    }

    // ── v0.9.1.2 F11 — markdown table rendering ──────────────────────

    /// Find a line whose joined text contains every substring in `needles`.
    fn find_line<'a>(lines: &'a [Line<'a>], needles: &[&str]) -> Option<&'a Line<'a>> {
        lines.iter().find(|l| {
            let t = line_text(l);
            needles.iter().all(|n| t.contains(n))
        })
    }

    #[test]
    fn render_markdown_table_basic_v0912() {
        // Header + 2 body rows → expect 4 output lines (header, separator,
        // 2 body rows). Header cells must be bold; separator must carry
        // `─` and `┼` box-drawing chars.
        let src = "| # | Repo |\n|---|------|\n| 1 | foo  |\n| 2 | bar  |\n";
        let (lines, _) = render_markdown(src, &theme());
        assert_eq!(
            lines.len(),
            4,
            "expected exactly 4 lines (header + sep + 2 body), got {}: {:?}",
            lines.len(),
            lines.iter().map(line_text).collect::<Vec<_>>()
        );
        // Header row contains both header texts and uses pipe separators.
        let header_text = line_text(&lines[0]);
        assert!(
            header_text.contains('│'),
            "header missing pipe separator: {header_text:?}"
        );
        assert!(
            header_text.contains('#'),
            "header missing '#': {header_text:?}"
        );
        assert!(
            header_text.contains("Repo"),
            "header missing 'Repo': {header_text:?}"
        );
        // At least one header span must be bold.
        let header_bold = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(header_bold, "no bold span in header row");
        // Separator row contains the box-drawing chars.
        let sep_text = line_text(&lines[1]);
        assert!(
            sep_text.contains('─'),
            "separator missing '─': {sep_text:?}"
        );
        assert!(
            sep_text.contains('┼'),
            "separator missing '┼': {sep_text:?}"
        );
        // Body rows contain content + pipe separators.
        assert!(line_text(&lines[2]).contains("foo"));
        assert!(line_text(&lines[3]).contains("bar"));
        // And critically: no raw `|...|` text leaked through unrendered.
        // The visual table uses U+2502 `│`, not ASCII `|`.
        for l in &lines {
            assert!(
                !line_text(l).contains('|'),
                "raw ASCII pipe leaked into output: {:?}",
                line_text(l)
            );
        }
    }

    #[test]
    fn render_markdown_table_with_alignments_v0912() {
        // `:---:` ⇒ center, `---:` ⇒ right. The header cell content is
        // 1-char wide; the column is widened by the wider body cell, so
        // center alignment must place leading padding before the header
        // and right alignment must place all padding before it.
        let src = "| A | B |\n|:---:|---:|\n| longA | longB |\n";
        let (lines, _) = render_markdown(src, &theme());
        // 3 lines: header, separator, 1 body row.
        assert_eq!(
            lines.len(),
            3,
            "got {:?}",
            lines.iter().map(line_text).collect::<Vec<_>>()
        );
        let header = line_text(&lines[0]);
        // Header text positions: col A is centred, col B is right-aligned.
        // With column width 5 (longA / longB), centred "A" gets 2 spaces
        // before and 2 after (plus the 1-space inner pad).
        // Layout: "│   A   │     B │"
        // Just check both letters are present and that A appears with
        // whitespace on both sides while B has whitespace only before it.
        let a_idx = header.find('A').expect("header A position");
        let b_idx = header.find('B').expect("header B position");
        // Char immediately after A must be a space (centred alignment).
        assert!(
            header[a_idx..].chars().nth(1) == Some(' '),
            "expected centred A with trailing space, got header={header:?}"
        );
        // Char immediately after B must NOT be a space (right alignment
        // pushes B flush to the right inner padding boundary; the only
        // space after B is the standard 1-char inner pad, then the pipe).
        // We accept exactly one trailing space then a pipe.
        let after_b: String = header[b_idx..].chars().skip(1).take(2).collect();
        assert_eq!(
            after_b, " │",
            "expected right-aligned B with ' │' trailer, got {after_b:?} in {header:?}"
        );
    }

    #[test]
    fn render_markdown_table_with_inline_links_v0912() {
        // `| [foo](https://x) | 1 |` — the link URL must be captured in
        // the returned URL vec, and the anchor text must be styled inside
        // the cell with `theme.link` fg.
        let t = theme();
        let src = "| name | n |\n|------|---|\n| [foo](https://x) | 1 |\n";
        let (lines, urls) = render_markdown(src, &t);
        assert_eq!(
            urls,
            vec!["https://x".to_string()],
            "link URL must be collected from inside a table cell"
        );
        // The link anchor "foo" must appear styled with theme.link in some
        // span on the body row (last line).
        let body = lines.last().expect("body row");
        let anchor = body
            .spans
            .iter()
            .find(|s| s.content == "foo")
            .expect("anchor span 'foo' inside cell");
        assert_eq!(
            anchor.style.fg,
            Some(t.link),
            "link anchor inside table cell missing link fg"
        );
        // And the URL trailer must also live inside the cell (sandwiched
        // between cell-inner padding and the pipe separator). v0.9.1.4:
        // OSC 8 opener span ALSO carries the URL substring — skip raw
        // escape spans (they start with `\x1b`).
        let trailer = body
            .spans
            .iter()
            .find(|s| s.content.contains("https://x") && !s.content.starts_with('\x1b'))
            .expect("url trailer span");
        assert_eq!(trailer.style.fg, Some(t.text_dim));
    }

    #[test]
    fn render_markdown_table_unequal_cell_counts_v0912() {
        // Defensive: a body row with fewer cells than the header. The
        // renderer must pad to column count, never panic, and the output
        // must still be exactly N+2 lines (header + sep + N body rows).
        // Note: pulldown-cmark's GFM table parser is strict about cell
        // count, but markdown sources from LLMs are not — we render
        // whatever cells the parser yields, padded.
        let src = "| A | B | C |\n|---|---|---|\n| 1 | 2 | 3 |\n";
        let (lines, _) = render_markdown(src, &theme());
        // Sanity: 3 lines (header + sep + body).
        assert_eq!(lines.len(), 3);
        // And a degenerate single-cell row inside a 3-col table doesn't panic.
        // We simulate the post-parse condition by feeding a fresh src with a
        // missing pipe; pulldown-cmark may emit a row with fewer cells.
        let degenerate = "| A | B | C |\n|---|---|---|\n| only |\n";
        let (lines2, _) = render_markdown(degenerate, &theme());
        // Don't crash — that's the contract. Either the parser dropped the
        // row, or it emitted a short row that we padded. Both are fine.
        assert!(lines2.iter().any(|l| line_text(l).contains('│')));
    }

    // ── v0.9.1.2 F11-followup — bullet-list fallback for wide tables ──

    /// The screenshot bug source: a 5-column table whose first body cell
    /// contains a 60-column URL. On a real 100-column terminal the natural
    /// width is far over the budget so the box-drawing renderer produces
    /// misaligned columns and wrapped pipes. The fallback must render
    /// every body row as `• <first cell>` + an indented continuation line
    /// of `<header>: <cell> · <header>: <cell>` pairs, with no `│`
    /// box-drawing chars anywhere in the output.
    #[test]
    fn wide_table_falls_back_to_bullet_list_v0913() {
        let src = "\
| Project | Stars | Stack | License | Notes |
|---------|-------|-------|---------|-------|
| gitroomhq/postiz-app | 30.9k | TypeScript / Next.js | AGPL-3.0 | The clear leader. |
";
        // Budget is 60 cols — well below the table's natural width
        // (sum of cell widths for the body row alone is > 60).
        let (lines, _) = render_markdown_with_width(src, &theme(), 60);
        let dump = all_text(&lines);
        assert!(
            !dump.contains('│'),
            "wide table fallback should NOT contain box-drawing `│`; got:\n{dump}"
        );
        assert!(
            !dump.contains('─')
                && !dump.contains('┼')
                && !dump.contains('├')
                && !dump.contains('┤'),
            "wide table fallback should NOT contain any box-drawing chars; got:\n{dump}"
        );
        assert!(
            dump.contains('•'),
            "wide table fallback must emit bullet markers; got:\n{dump}"
        );
        // The first body cell becomes the bullet title.
        assert!(
            dump.contains("gitroomhq/postiz-app"),
            "first-cell title missing from bullet line; got:\n{dump}"
        );
        // Header labels prefix the continuation cells.
        assert!(
            dump.contains("Stars: 30.9k"),
            "expected `Stars: 30.9k` prefix in continuation; got:\n{dump}"
        );
        assert!(
            dump.contains("License: AGPL-3.0"),
            "expected `License: AGPL-3.0` prefix in continuation; got:\n{dump}"
        );
    }

    /// A narrow table that fits the budget must still render via the
    /// existing box-drawing path — the fallback should only trigger when
    /// the table actually overflows.
    #[test]
    fn narrow_table_renders_as_box_drawing_v0913() {
        let src = "| # | Repo |\n|---|------|\n| 1 | foo  |\n| 2 | bar  |\n";
        let (lines, _) = render_markdown_with_width(src, &theme(), 100);
        let dump = all_text(&lines);
        assert!(
            dump.contains('│'),
            "narrow table must still render with box-drawing `│`; got:\n{dump}"
        );
        assert!(
            dump.contains('─'),
            "narrow table must still render with separator `─`; got:\n{dump}"
        );
        // No bullet rendering at all — the fallback path should be cold.
        assert!(
            !dump.contains('•'),
            "narrow table must NOT use bullet fallback; got:\n{dump}"
        );
    }

    /// In the bullet fallback, the *first* cell of each row is the bullet
    /// title — it does NOT receive a `<header>:` prefix. Subsequent cells
    /// do. The test feeds a 3-col table whose first cell is "RowOne" and
    /// asserts no `Project: ` (or whatever the first header was) appears
    /// in front of `RowOne`.
    #[test]
    fn bullet_fallback_uses_first_cell_as_title_v0913() {
        let src = "\
| Project | Stars | Stack |
|---------|-------|-------|
| RowOne  | 1.0k  | Rust  |
";
        let (lines, _) = render_markdown_with_width(src, &theme(), 20);
        // Locate the bullet line — it's the first line containing `•`.
        let bullet_line = lines
            .iter()
            .map(line_text)
            .find(|t| t.contains('•'))
            .expect("bullet line must exist in fallback output");
        // The bullet line should be exactly `• RowOne` (modulo styling
        // spans the string view ignores).
        assert!(
            bullet_line.contains("RowOne"),
            "bullet line must contain first-cell title; got {bullet_line:?}"
        );
        assert!(
            !bullet_line.contains("Project:"),
            "first cell must NOT be prefixed by its header label; got {bullet_line:?}"
        );
    }

    /// The continuation cells in the bullet fallback are joined with
    /// ` · ` (a middot with spaces) so the row reads as one logical
    /// record. This separator MUST appear between subsequent cells.
    #[test]
    fn bullet_fallback_joins_remaining_cells_with_middot_v0913() {
        let src = "\
| Project | Stars | Stack |
|---------|-------|-------|
| RowOne  | 1.0k  | Rust  |
";
        let (lines, _) = render_markdown_with_width(src, &theme(), 20);
        let dump = all_text(&lines);
        assert!(
            dump.contains(" · "),
            "continuation cells must be joined by ` · `; got:\n{dump}"
        );
        // And the two non-title cells both appear with their header prefixes.
        assert!(dump.contains("Stars: 1.0k"));
        assert!(dump.contains("Stack: Rust"));
    }

    #[test]
    fn render_markdown_strikethrough_v0912() {
        // GFM `~~text~~` ⇒ CROSSED_OUT modifier. ENABLE_STRIKETHROUGH was
        // turned on alongside ENABLE_TABLES, and the strikethrough arms
        // were wired to push/pop the modifier.
        let src = "~~gone~~";
        let (lines, _) = render_markdown(src, &theme());
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "gone")
            .expect("strikethrough text span");
        assert!(
            span.style.add_modifier.contains(Modifier::CROSSED_OUT),
            "strikethrough span missing CROSSED_OUT modifier: {:?}",
            span.style
        );
    }

    // ── v0.9.1.3 polish — blank line between top-level list items ──

    /// Sean's bug screenshot: numbered list with continuation paragraphs
    /// rendered with NO breathing room between items. After the fix,
    /// every top-level item end emits a blank Line so the next item
    /// starts on its own visual block. We only assert the relative
    /// position (continuation → blank → "2. bar"), not exact line indices,
    /// so the test survives future spacing tweaks elsewhere in the
    /// renderer.
    #[test]
    fn top_level_list_items_have_blank_line_between_v0913() {
        let src = "1. foo\n   continuation\n\n2. bar\n";
        let (lines, _) = render_markdown(src, &theme());
        // Locate the continuation line and the "2. bar" line.
        let cont_ix = lines
            .iter()
            .position(|l| line_text(l).contains("continuation"))
            .expect("continuation line present");
        let bar_ix = lines
            .iter()
            .position(|l| line_text(l).contains("bar"))
            .expect("bar line present");
        assert!(
            bar_ix > cont_ix,
            "expected 'bar' to follow 'continuation'; got cont_ix={cont_ix} bar_ix={bar_ix}"
        );
        // At least one visually-empty line must sit strictly between.
        let has_gap = lines[cont_ix + 1..bar_ix]
            .iter()
            .any(line_is_visually_empty);
        assert!(
            has_gap,
            "expected a blank Line between item 1's continuation and item 2; \
             got:\n{}",
            lines
                .iter()
                .enumerate()
                .map(|(i, l)| format!("  {i}: {:?}", line_text(l)))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// Inside a single outer item, the nested bullets must stay TIGHT —
    /// no gap between sibling inner-list items. The inner list has no
    /// blank line between its items in source, so pulldown emits their
    /// text inline (no Paragraph wrapper) and the v0.9.2 W7 S8
    /// loose-detection keeps `list_loose` false for that level → no gap.
    #[test]
    fn nested_list_items_stay_tight_v0913() {
        let src = "1. outer\n   - inner a\n   - inner b\n";
        let (lines, _) = render_markdown(src, &theme());
        let a_ix = lines
            .iter()
            .position(|l| line_text(l).contains("inner a"))
            .expect("inner a line present");
        let b_ix = lines
            .iter()
            .position(|l| line_text(l).contains("inner b"))
            .expect("inner b line present");
        assert!(
            b_ix > a_ix,
            "expected 'inner b' to follow 'inner a'; got a_ix={a_ix} b_ix={b_ix}"
        );
        // ZERO blank lines may sit between the two nested bullets.
        let blanks_between = lines[a_ix + 1..b_ix]
            .iter()
            .filter(|l| line_is_visually_empty(l))
            .count();
        assert_eq!(
            blanks_between,
            0,
            "nested bullets must stay tight (no blank Line between); got:\n{}",
            lines
                .iter()
                .enumerate()
                .map(|(i, l)| format!("  {i}: {:?}", line_text(l)))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // ── v0.9.2 W7 (S8) — tight-by-default lists, loose opt-in ──────────

    /// A CommonMark *tight* list (no blank lines between items in source)
    /// renders with ZERO blank gaps between the items. This is the S8
    /// default and the case the v0.9.1.3 nesting heuristic got wrong
    /// (it blanked every top-level list regardless of source spacing).
    #[test]
    fn tight_list_has_no_blank_gaps_between_items_v092() {
        let (lines, _) = render_markdown("- a\n- b\n- c", &theme());
        let blanks = lines.iter().filter(|l| line_is_visually_empty(l)).count();
        assert_eq!(
            blanks,
            0,
            "tight list must render with no blank gaps; got:\n{}",
            lines
                .iter()
                .enumerate()
                .map(|(i, l)| format!("  {i}: {:?}", line_text(l)))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// A CommonMark *loose* list (blank lines between items in source,
    /// which makes pulldown wrap each item's text in a block-level
    /// Paragraph) keeps the blank gaps between items.
    #[test]
    fn loose_list_keeps_blank_gaps_v092() {
        let (lines, _) = render_markdown("- a\n\n- b\n\n- c", &theme());
        assert!(
            lines.iter().any(line_is_visually_empty),
            "loose list must keep at least one blank gap; got:\n{}",
            lines
                .iter()
                .enumerate()
                .map(|(i, l)| format!("  {i}: {:?}", line_text(l)))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// A tight ordered list is tight too — the looseness signal is the
    /// Paragraph wrapper, not the bullet-vs-number distinction.
    #[test]
    fn tight_ordered_list_has_no_blank_gaps_v092() {
        let (lines, _) = render_markdown("1. one\n2. two\n3. three", &theme());
        let blanks = lines.iter().filter(|l| line_is_visually_empty(l)).count();
        assert_eq!(
            blanks, 0,
            "tight ordered list must render with no blank gaps"
        );
    }

    /// A tight outer list whose item contains a nested *loose* list keeps
    /// the outer level tight while the inner items get their gaps. This
    /// proves the per-level `list_loose` stack isolates looseness to the
    /// level that actually carries block paragraphs: making the inner
    /// list loose must NOT add the outer level a per-item gap.
    #[test]
    fn loose_inner_list_does_not_loosen_tight_outer_v092() {
        // Inner list under `outer1` is loose (blank between `in a`/`in b`);
        // the outer list is tight.
        let loose_inner = "- outer1\n  - in a\n\n  - in b\n- outer2";
        let (lines, _) = render_markdown(loose_inner, &theme());
        let ia = lines
            .iter()
            .position(|l| line_text(l).contains("in a"))
            .expect("in a present");
        let ib = lines
            .iter()
            .position(|l| line_text(l).contains("in b"))
            .expect("in b present");
        // Inner loose list keeps its gap between `in a` and `in b`.
        assert!(
            lines[ia + 1..ib].iter().any(line_is_visually_empty),
            "loose inner list must keep a gap between its items"
        );
        // The OUTER level stays tight: the line directly after `outer1`
        // is its inner content (`in a`), NOT an outer-introduced blank.
        let o1 = lines
            .iter()
            .position(|l| line_text(l).contains("outer1"))
            .expect("outer1 present");
        assert!(
            !line_is_visually_empty(&lines[o1 + 1]),
            "tight outer item must not be followed by a blank; got:\n{}",
            lines
                .iter()
                .enumerate()
                .map(|(i, l)| format!("  {i}: {:?}", line_text(l)))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let loose_inner_blanks = lines.iter().filter(|l| line_is_visually_empty(l)).count();

        // Control: make the OUTER list loose too (blank-separated outer
        // items). That must add MORE blanks than the inner-only-loose
        // variant — proving the outer level's looseness is tracked
        // independently and was correctly OFF in the first render.
        let outer_loose = "- outer1\n\n  - in a\n\n  - in b\n\n- outer2";
        let (ol, _) = render_markdown(outer_loose, &theme());
        let outer_loose_blanks = ol.iter().filter(|l| line_is_visually_empty(l)).count();
        assert!(
            outer_loose_blanks > loose_inner_blanks,
            "loosening the OUTER list must add gaps the tight-outer render lacks \
             (outer_loose={outer_loose_blanks}, inner_only={loose_inner_blanks})"
        );
    }
}
