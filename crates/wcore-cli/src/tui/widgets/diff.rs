//! Syntax-highlighted diff widget — renders a `DiffModel` line diff.
//!
//! FROZEN Wave-0 signature; T0.4 fills the body. The widget computes an
//! LCS line diff of `old` -> `new`, applies `syntect` syntax highlighting
//! per line, and caches the highlighted result per file-version so a
//! repeated render of an unchanged `DiffModel` skips the highlight pass.
//!
//! ## Hardening (W3-B)
//!
//! Two bounds keep the widget safe on a long, busy session:
//!
//!  * **Bounded LCS.** The line diff is an `O(m·n)` LCS over an
//!    `(m+1)·(n+1)` matrix. For a normal edit that is trivial, but a
//!    *huge* `old`/`new` (a multi-megabyte file, a giant tool output
//!    captured as a write) would allocate gigabytes and freeze the draw.
//!    [`compute_diff`] caps the line count at [`MAX_DIFF_LINES`]: past the
//!    cap it switches to a linear positional diff that allocates nothing
//!    quadratic, so the render time and memory stay bounded regardless of
//!    input size.
//!  * **Bounded highlight cache.** The per-file-version highlight cache is
//!    capped at [`MAX_CACHE_ENTRIES`] with FIFO eviction. Without the cap
//!    every distinct edit preview in a long session would leak one cache
//!    entry forever.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::tui::app::DiffModel;
use crate::tui::theme::Theme;

/// One classified line in the computed diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    /// Present in both `old` and `new`.
    Context,
    /// Only in `new` — an addition.
    Add,
    /// Only in `old` — a removal.
    Removal,
}

/// A diff line: its kind plus the source text (no trailing newline).
struct DiffLine {
    kind: DiffKind,
    text: String,
}

/// Render a syntax-highlighted line diff of a `DiffModel` (old vs new).
///
/// Layout mirrors the mockup's `.diff`: a header row carrying the file
/// path and an add/remove count, then the diff body. Each body line has a
/// sign column (`+`/`-`/space), the highlighted code, and an
/// add/remove/context background tint.
///
/// FROZEN Wave-0 contract.
pub fn diff_view(f: &mut Frame, area: Rect, diff: &DiffModel, t: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let lines = compute_diff(&diff.old, &diff.new);
    let (adds, removals) = count_changes(&lines);

    // Header: the file path + the change counts.
    let title = Line::from(vec![
        Span::styled(
            format!(" {} ", diff.path),
            Style::default()
                .bg(t.surface)
                .fg(t.text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" +{adds} "),
            Style::default().bg(t.surface).fg(t.success),
        ),
        Span::styled(
            format!("-{removals} "),
            Style::default().bg(t.surface).fg(t.error),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.bg))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // The body is the shared line-builder — the SAME lines a permission
    // component embeds inline via `diff_lines`. The Frame renderer wraps
    // them in its bordered block; the permission body uses them raw.
    let body = diff_lines(diff, inner.width, t);

    let para = Paragraph::new(body).style(Style::default().bg(t.bg));
    f.render_widget(para, inner);
}

/// Build the diff BODY as a `Vec<Line>` — the inner line-building of
/// [`diff_view`] without the `Frame`, the bordered block, or the header.
///
/// This is the line-builder the v0.9.2 permission components
/// (FileEdit/FileWrite/NotebookEdit) embed inline: a `body() -> Vec<Line>`
/// cannot paint into a `Frame`, so it calls `diff_lines` to get the same
/// classified-and-highlighted rows `diff_view` renders, then composes them
/// into the shared `PermissionDialog` chrome. Both paths share one
/// line-builder so the inline diff and the widget never drift.
///
/// Each returned line carries a `+`/`-`/space sign column and the
/// syntect-highlighted code with an add/remove/context background tint.
/// `width` is reserved for future per-line wrapping; today the underlying
/// `render_line` does not wrap, so it is unused for layout — kept in the
/// signature so the permission callers pass their content width without a
/// later signature break.
pub fn diff_lines(diff: &DiffModel, width: u16, t: &Theme) -> Vec<Line<'static>> {
    let _ = width; // reserved for future per-line wrapping (see doc).
    let lines = compute_diff(&diff.old, &diff.new);
    let highlighted = highlight_cached(&diff.path, diff, &lines);
    lines
        .iter()
        .zip(highlighted.iter())
        .map(|(dl, spans)| render_line(dl, spans, t))
        .collect()
}

/// Build one rendered `Line`: a sign column + the highlighted code, with
/// an add/remove/context background tint.
fn render_line(dl: &DiffLine, code: &[(SynStyle, String)], t: &Theme) -> Line<'static> {
    let (sign, sign_color, bg) = match dl.kind {
        DiffKind::Add => ("+ ", t.success, tint(t.success)),
        DiffKind::Removal => ("- ", t.error, tint(t.error)),
        DiffKind::Context => ("  ", t.text_muted, t.bg),
    };

    let mut spans = vec![Span::styled(sign, Style::default().bg(bg).fg(sign_color))];

    // Removals render dimmed (struck-through visually by the red tint);
    // adds and context carry the syntect colors.
    for (syn, text) in code {
        let fg = syn_to_ratatui(syn);
        spans.push(Span::styled(text.clone(), Style::default().bg(bg).fg(fg)));
    }

    Line::from(spans)
}

/// A subtle background tint for an add/remove row — the base accent at
/// low intensity so the row reads as a band, not a block of solid color.
fn tint(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            // Mix ~18% of the accent over the near-black diff background.
            let mix = |ch: u8| ((ch as u16 * 18 + 0x0d * 82) / 100) as u8;
            Color::Rgb(mix(r), mix(g), mix(b))
        }
        // For the uncolored theme keep the terminal default.
        other => other,
    }
}

/// Convert a syntect highlight style to a ratatui foreground color.
fn syn_to_ratatui(s: &SynStyle) -> Color {
    Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b)
}

/// Count the added and removed lines in a computed diff.
fn count_changes(lines: &[DiffLine]) -> (usize, usize) {
    let adds = lines.iter().filter(|l| l.kind == DiffKind::Add).count();
    let removals = lines.iter().filter(|l| l.kind == DiffKind::Removal).count();
    (adds, removals)
}

// ─────────────────────────────────────────────────────────────────────────
// Line diff — a Myers-style LCS over whole lines
// ─────────────────────────────────────────────────────────────────────────

/// Per-side line cap for the quadratic LCS path. Above this on either side
/// the diff falls back to a linear positional diff: the `(m+1)·(n+1)`
/// `usize` matrix the LCS allocates would be `> ~2.5 GiB` at, say,
/// 18k×18k lines, which would freeze the render and risk an OOM. A diff
/// that large is never meaningfully *readable* anyway — the bounded path
/// keeps the widget responsive and the transcript scrollable.
const MAX_DIFF_LINES: usize = 4_000;

/// Compute a line diff of `old` -> `new`.
///
/// For inputs within [`MAX_DIFF_LINES`] on both sides this is an exact LCS
/// (common lines `Context`, the rest `Removal`/`Add`). For a larger input
/// it degrades to [`positional_diff`] — a linear, allocation-bounded diff
/// — so a pathologically large `DiffModel` can never freeze the draw.
fn compute_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let (m, n) = (old_lines.len(), new_lines.len());

    // Guard the quadratic LCS: a huge file diff falls back to the linear
    // path rather than allocating an `(m+1)·(n+1)` matrix.
    if m > MAX_DIFF_LINES || n > MAX_DIFF_LINES {
        return positional_diff(&old_lines, &new_lines);
    }

    // LCS length table.
    let mut lcs = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            lcs[i][j] = if old_lines[i] == new_lines[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    // Walk the table to emit the diff in source order.
    let mut out = Vec::with_capacity(m + n);
    let (mut i, mut j) = (0, 0);
    while i < m && j < n {
        if old_lines[i] == new_lines[j] {
            out.push(DiffLine {
                kind: DiffKind::Context,
                text: old_lines[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(DiffLine {
                kind: DiffKind::Removal,
                text: old_lines[i].to_string(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                kind: DiffKind::Add,
                text: new_lines[j].to_string(),
            });
            j += 1;
        }
    }
    while i < m {
        out.push(DiffLine {
            kind: DiffKind::Removal,
            text: old_lines[i].to_string(),
        });
        i += 1;
    }
    while j < n {
        out.push(DiffLine {
            kind: DiffKind::Add,
            text: new_lines[j].to_string(),
        });
        j += 1;
    }
    out
}

/// A linear, allocation-bounded diff for inputs too large for the LCS
/// path. It is not minimal — it pairs lines positionally — but it never
/// allocates anything quadratic and runs in `O(m + n)`. The intent is a
/// *safe* render of a huge `DiffModel`, not an optimal one: an edit that
/// touches thousands of lines is a wall of changes either way.
///
/// Equal lines at the same index are `Context`; a differing pair becomes a
/// `Removal` then an `Add`; the trailing surplus on the longer side is all
/// `Removal` or all `Add`.
fn positional_diff(old_lines: &[&str], new_lines: &[&str]) -> Vec<DiffLine> {
    let common = old_lines.len().min(new_lines.len());
    let mut out = Vec::with_capacity(old_lines.len() + new_lines.len());
    for k in 0..common {
        if old_lines[k] == new_lines[k] {
            out.push(DiffLine {
                kind: DiffKind::Context,
                text: old_lines[k].to_string(),
            });
        } else {
            out.push(DiffLine {
                kind: DiffKind::Removal,
                text: old_lines[k].to_string(),
            });
            out.push(DiffLine {
                kind: DiffKind::Add,
                text: new_lines[k].to_string(),
            });
        }
    }
    for line in old_lines.iter().skip(common) {
        out.push(DiffLine {
            kind: DiffKind::Removal,
            text: line.to_string(),
        });
    }
    for line in new_lines.iter().skip(common) {
        out.push(DiffLine {
            kind: DiffKind::Add,
            text: line.to_string(),
        });
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────
// syntect highlighting — loaded once, cached per file-version
// ─────────────────────────────────────────────────────────────────────────

/// The syntect syntax/theme sets, loaded once on first diff render. Both
/// loads are non-trivial (parse the embedded definition dumps), so they
/// live behind a `OnceLock`.
struct Highlighter {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

/// The process-wide highlighter handle.
fn highlighter() -> &'static Highlighter {
    static HL: OnceLock<Highlighter> = OnceLock::new();
    HL.get_or_init(|| Highlighter {
        syntaxes: SyntaxSet::load_defaults_newlines(),
        themes: ThemeSet::load_defaults(),
    })
}

/// Maximum live entries in the highlight cache. A long session edits many
/// distinct files; without a cap every file-version would leak one entry
/// forever. 64 covers any realistic on-screen working set (only a handful
/// of diffs are ever visible at once) while bounding the memory.
const MAX_CACHE_ENTRIES: usize = 64;

/// One highlight result: per-`DiffLine` styled spans.
type Highlighted = Vec<Vec<(SynStyle, String)>>;

/// A FIFO-bounded highlight cache: `file-version hash` -> highlighted
/// lines. A `DiffModel` is immutable per file version, so the key folds in
/// the path and both buffers; an unchanged diff hits the cache and skips
/// the `syntect` pass entirely. When the cache is full the oldest entry is
/// evicted, so the memory footprint is bounded on a long session.
struct HighlightCache {
    /// The key→result table.
    entries: std::collections::HashMap<u64, Highlighted>,
    /// Insertion order, for FIFO eviction once `entries` hits the cap.
    order: VecDeque<u64>,
}

impl HighlightCache {
    fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Fetch a cached result by key.
    fn get(&self, key: u64) -> Option<&Highlighted> {
        self.entries.get(&key)
    }

    /// Insert a result, evicting the oldest entry if at capacity. A
    /// re-insert of an existing key just refreshes the value (the key
    /// keeps its original FIFO position — a re-render is not a "use").
    fn insert(&mut self, key: u64, value: Highlighted) {
        if self.entries.insert(key, value).is_some() {
            return; // key already present — value refreshed, order intact.
        }
        self.order.push_back(key);
        while self.order.len() > MAX_CACHE_ENTRIES {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
            }
        }
    }

    /// Live entry count — used by tests to assert the bound holds.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The process-wide highlight cache.
fn cache() -> &'static Mutex<HighlightCache> {
    static CACHE: OnceLock<Mutex<HighlightCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HighlightCache::new()))
}

/// A stable key for a `DiffModel` — path + old + new fold into one hash.
fn version_key(diff: &DiffModel) -> u64 {
    let mut h = DefaultHasher::new();
    diff.path.hash(&mut h);
    diff.old.hash(&mut h);
    diff.new.hash(&mut h);
    h.finish()
}

/// Highlight every line of the diff, caching by file-version. The result
/// is parallel to `lines`: one `Vec<(style, text)>` per `DiffLine`.
///
/// On a repeated render of an unchanged `DiffModel` this returns the
/// cached result and skips `syntect` entirely — that hit is what keeps the
/// 30fps loop inside its frame budget when a diff card stays on screen.
fn highlight_cached(path: &str, diff: &DiffModel, lines: &[DiffLine]) -> Highlighted {
    let key = version_key(diff);
    if let Ok(guard) = cache().lock()
        && let Some(hit) = guard.get(key)
    {
        return hit.clone();
    }

    let computed = highlight_lines(path, lines);
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, computed.clone());
    }
    computed
}

/// Run `syntect` over each diff line, returning per-line styled spans.
fn highlight_lines(path: &str, lines: &[DiffLine]) -> Highlighted {
    let hl = highlighter();
    let syntax = syntax_for(&hl.syntaxes, path);
    // A dark theme so highlighted colors read on the near-black diff bg.
    let theme = &hl.themes.themes["base16-ocean.dark"];

    // `HighlightLines` is stateful (it carries the parse scope across
    // lines), so highlight the whole diff in source order with one
    // highlighter — context lines keep the parser's scope coherent.
    let mut h = HighlightLines::new(syntax, theme);
    lines
        .iter()
        .map(|dl| {
            // syntect wants a trailing newline for its line regexes.
            let owned = format!("{}\n", dl.text);
            match h.highlight_line(&owned, &hl.syntaxes) {
                Ok(ranges) => ranges
                    .into_iter()
                    .map(|(style, frag)| (style, frag.trim_end_matches('\n').to_string()))
                    .filter(|(_, frag)| !frag.is_empty())
                    .collect(),
                // On any highlight error fall back to the raw text so the
                // diff still renders — never panic in a draw path.
                Err(_) => vec![(SynStyle::default(), dl.text.clone())],
            }
        })
        .collect()
}

/// Resolve the syntect syntax for a file path by its extension, falling
/// back to plain text when the extension is unknown.
fn syntax_for<'a>(syntaxes: &'a SyntaxSet, path: &str) -> &'a SyntaxReference {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| syntaxes.find_syntax_by_extension(ext))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text())
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::app::DiffModel;
    use crate::tui::theme::Theme;

    fn render(diff: &DiffModel, t: &Theme, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| diff_view(f, f.area(), diff, t))
            .expect("render diff");
        let buf = terminal.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn line_diff_classifies_add_remove_context() {
        let lines = compute_diff("a\nb\nc\n", "a\nB\nc\n");
        let kinds: Vec<DiffKind> = lines.iter().map(|l| l.kind).collect();
        // `a` and `c` are common; `b` -> `B` is a removal then an add.
        assert_eq!(
            kinds,
            vec![
                DiffKind::Context,
                DiffKind::Removal,
                DiffKind::Add,
                DiffKind::Context
            ]
        );
        let (adds, rms) = count_changes(&lines);
        assert_eq!((adds, rms), (1, 1));
    }

    #[test]
    fn line_diff_handles_pure_insertion() {
        let lines = compute_diff("a\n", "a\nb\nc\n");
        let (adds, rms) = count_changes(&lines);
        assert_eq!((adds, rms), (2, 0));
    }

    #[test]
    fn line_diff_handles_pure_deletion() {
        let lines = compute_diff("a\nb\nc\n", "b\n");
        let (adds, rms) = count_changes(&lines);
        assert_eq!((adds, rms), (0, 2));
    }

    #[test]
    fn diff_lines_returns_body_lines_without_a_frame() {
        let t = Theme::hearth();
        let model = DiffModel {
            path: "src/main.rs".into(),
            old: "fn main() {}\n".into(),
            new: "fn main() { println!(\"hi\"); }\n".into(),
        };
        let lines = diff_lines(&model, 80, &t);
        assert!(!lines.is_empty());
        // First body line should carry a +/- sign column, not a border char.
        let first: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(first.starts_with('+') || first.starts_with('-') || first.starts_with(' '));
    }

    #[test]
    fn diff_view_renders_header_with_path_and_counts() {
        let diff = DiffModel {
            path: "src/main.rs".into(),
            old: "let x = 1;\n".into(),
            new: "let x = 2;\n".into(),
        };
        let rows = render(&diff, &Theme::hearth(), 60, 6);
        let header = &rows[0];
        assert!(header.contains("src/main.rs"), "path missing: {header}");
        assert!(header.contains("+1"), "add count missing: {header}");
        assert!(header.contains("-1"), "remove count missing: {header}");
    }

    #[test]
    fn diff_view_highlights_a_rust_snippet() {
        // A Rust diff must drive the `syntect` highlighter — the `fn`
        // keyword should not render in the plain text color.
        let diff = DiffModel {
            path: "lib.rs".into(),
            old: "fn old() {}\n".into(),
            new: "fn renamed() -> u32 { 42 }\n".into(),
        };
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).expect("test terminal");
        terminal
            .draw(|f| diff_view(f, f.area(), &diff, &t))
            .expect("render");
        let buf = terminal.backend().buffer();

        // Find the added line; its `fn` keyword must carry a syntect
        // color distinct from the plain `text` foreground.
        let mut found_colored_keyword = false;
        for y in 1..8 {
            let row: String = (0..60).map(|x| buf[(x, y)].symbol()).collect();
            if row.contains("renamed") {
                // The 'f' of `fn` sits at the code start (after sign col +
                // border inset). Scan the row for any non-default,
                // non-plain syntax color.
                for x in 0..60 {
                    let cell = &buf[(x, y)];
                    if cell.symbol() == "f" && !matches!(cell.fg, Color::Reset) && cell.fg != t.text
                    {
                        found_colored_keyword = true;
                    }
                }
            }
        }
        assert!(
            found_colored_keyword,
            "syntect did not color the Rust keyword"
        );
    }

    #[test]
    fn diff_view_caches_highlight_by_file_version() {
        let diff = DiffModel {
            path: "cached.rs".into(),
            old: "fn a() {}\n".into(),
            new: "fn b() {}\n".into(),
        };
        let lines = compute_diff(&diff.old, &diff.new);
        // First call populates the cache; a second identical call must
        // return an equal result (the cache hit path).
        let first = highlight_cached(&diff.path, &diff, &lines);
        let second = highlight_cached(&diff.path, &diff, &lines);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            let at: Vec<&str> = a.iter().map(|(_, s)| s.as_str()).collect();
            let bt: Vec<&str> = b.iter().map(|(_, s)| s.as_str()).collect();
            assert_eq!(at, bt);
        }
        // The version key must be present in the shared cache.
        let key = version_key(&diff);
        assert!(cache().lock().unwrap().get(key).is_some());
    }

    #[test]
    fn highlight_cache_is_bounded_and_evicts_oldest() {
        // Insert well past the cap directly into a fresh cache and confirm
        // the live entry count never exceeds `MAX_CACHE_ENTRIES` — without
        // the bound a long session would leak one entry per edit forever.
        let mut c = HighlightCache::new();
        for k in 0..(MAX_CACHE_ENTRIES as u64 * 4) {
            c.insert(k, vec![vec![(SynStyle::default(), format!("line{k}"))]]);
            assert!(
                c.len() <= MAX_CACHE_ENTRIES,
                "cache exceeded its bound at key {k}"
            );
        }
        assert_eq!(c.len(), MAX_CACHE_ENTRIES);
        // FIFO: the earliest keys are gone, the most recent survive.
        assert!(c.get(0).is_none(), "oldest entry should be evicted");
        let newest = MAX_CACHE_ENTRIES as u64 * 4 - 1;
        assert!(c.get(newest).is_some(), "newest entry must be retained");
    }

    #[test]
    fn highlight_cache_reinsert_refreshes_without_growing() {
        // Re-inserting an existing key (a re-render of the same diff) must
        // update the value in place, not grow the cache or the order list.
        let mut c = HighlightCache::new();
        c.insert(7, vec![vec![(SynStyle::default(), "v1".into())]]);
        c.insert(7, vec![vec![(SynStyle::default(), "v2".into())]]);
        assert_eq!(c.len(), 1);
        assert_eq!(c.order.len(), 1);
        let got = &c.get(7).unwrap()[0][0].1;
        assert_eq!(got, "v2");
    }

    #[test]
    fn large_diff_falls_back_to_the_bounded_positional_path() {
        // A diff far past `MAX_DIFF_LINES` on each side must NOT allocate
        // the quadratic LCS matrix (gigabytes) — it takes the linear
        // positional path. The assertion here is that it returns promptly
        // without OOM/panic; the line classification is best-effort.
        let over = MAX_DIFF_LINES + 200;
        let big_old: String = (0..over).map(|i| format!("old line {i}\n")).collect();
        let big_new: String = (0..over).map(|i| format!("new line {i}\n")).collect();
        let lines = compute_diff(&big_old, &big_new);
        // Every line differs positionally → one removal + one add each.
        assert_eq!(lines.len(), over * 2);
    }

    #[test]
    fn positional_diff_classifies_context_and_trailing_surplus() {
        // Equal lines at the same index are context; the longer side's
        // tail is all additions.
        let old: Vec<&str> = vec!["a", "b"];
        let new: Vec<&str> = vec!["a", "B", "c"];
        let lines = positional_diff(&old, &new);
        let kinds: Vec<DiffKind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffKind::Context, // a == a
                DiffKind::Removal, // b -> B
                DiffKind::Add,
                DiffKind::Add, // trailing c
            ]
        );
    }

    #[test]
    fn diff_view_renders_a_huge_diff_without_freezing() {
        // The widget's draw path must survive an oversized `DiffModel` —
        // this is the "large tool output captured as an edit" case. With
        // the input just past `MAX_DIFF_LINES` the fallback positional
        // path is taken; the widget must render without panic or
        // unbounded work. A `.txt` path keeps the test fast (the LCS
        // bound, not syntect throughput, is what is under test).
        let over = MAX_DIFF_LINES + 100;
        let huge_old: String = (0..over).map(|i| format!("old {i}\n")).collect();
        let huge_new: String = (0..over).map(|i| format!("new {i}\n")).collect();
        let diff = DiffModel {
            path: "huge.txt".into(),
            old: huge_old,
            new: huge_new,
        };
        let rows = render(&diff, &Theme::hearth(), 80, 24);
        // The header still renders the path — the widget did not bail.
        assert!(rows[0].contains("huge.txt"));
    }

    #[test]
    fn diff_view_renders_with_no_color_theme() {
        let diff = DiffModel {
            path: "x.rs".into(),
            old: "a\n".into(),
            new: "b\n".into(),
        };
        // Must not panic; the uncolored theme still renders structure.
        let rows = render(&diff, &Theme::no_color(), 40, 6);
        assert!(rows[0].contains("x.rs"));
    }
}
