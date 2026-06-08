//! Per-turn "Sources" block.
//!
//! v0.9.0 W3 D4: when an assistant turn references URLs (either inline
//! markdown links in the streamed body, per W2 C1, or in tool-result
//! payloads, per W2 C2/C3 `extract_urls`), the bridge merges + dedupes
//! them into a `TurnElement::Sources(Vec<String>)` element. The
//! transcript renderer walks that variant through [`render_sources`] to
//! produce a small footer of citation lines after the body.
//!
//! ## Format
//!
//! ```text
//! Sources:
//!   1. example.com — https://example.com/path
//!   2. github.com — https://github.com/repo/issues/1
//! ```
//!
//! All lines render in `theme.link` so the footer reads as the
//! "interactive affordance" colour — distinct from body text and
//! distinct from the markdown heading colour.
//!
//! v0.9.2 W9 (S24): each URL is now OSC-8-wrapped so it is Cmd-click /
//! Ctrl-click able in terminals that honour the escape. The opener /
//! closer ride in their own (link-styled) spans so the visible width is
//! unaffected. `mailto:` URLs render plain (no escape, not clickable).
//!
//! ## Scope
//!
//! - Caps at 10 entries. Overflow is dropped silently in v0.9.0 — the
//!   block is a citation hint, not an exhaustive log.
//! - Empty input yields zero lines (no `Sources:` header) so a turn with
//!   no references renders cleanly without a trailing empty header.
//! - The bridge is responsible for dedup before calling — this widget
//!   trusts its input.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::render::osc8;
use crate::tui::theme::Theme;

/// The maximum number of citation entries the Sources block prints.
/// Past this point the footer would crowd the transcript; v0.9.0 drops
/// the overflow silently. Re-visit in v0.9.x when OSC 8 hyperlinks land
/// and we can pack more density into a tighter affordance.
const MAX_SOURCES: usize = 10;

/// Render a Sources block from a list of URLs.
///
/// Each row is `  N. <domain> — <url>` in `theme.link`. The first row
/// is a `Sources:` header in the same colour. Empty input returns an
/// empty `Vec` so the caller can append unconditionally without
/// producing a stray header line.
pub fn render_sources(urls: &[String], theme: &Theme) -> Vec<Line<'static>> {
    if urls.is_empty() {
        return Vec::new();
    }
    let style = Style::default().fg(theme.link);
    let mut lines = Vec::with_capacity(urls.len().min(MAX_SOURCES) + 1);
    lines.push(Line::from(Span::styled("Sources:", style)));
    for (i, url) in urls.iter().take(MAX_SOURCES).enumerate() {
        let domain = extract_domain(url).unwrap_or_else(|| url.clone());
        // `  N. <domain> — ` is plain prose; the URL trailer is the
        // clickable part.
        let prefix = Span::styled(format!("  {}. {} — ", i + 1, domain), style);
        // v0.9.2 W9 (S24): linkify the URL. mailto: stays plain (no
        // escape, not clickable); text already carrying an OSC 8 opener
        // is left untouched (nested guard). The opener/closer ride in
        // their own (link-styled) spans so the visible width is
        // unaffected — and so the NO_COLOR / theme-link assertions that
        // walk every span still see `theme.link` on each.
        if osc8::is_plain_only(url) || osc8::contains_osc8(url) {
            lines.push(Line::from(vec![prefix, Span::styled(url.clone(), style)]));
        } else {
            lines.push(Line::from(vec![
                prefix,
                Span::styled(osc8::open_seq(url), style),
                Span::styled(url.clone(), style),
                Span::styled(osc8::close_seq(), style),
            ]));
        }
    }
    lines
}

/// Cheap host extraction: split on `"://"` then take everything up to
/// the next `/`, `?` or `#`. Returns `None` if the input has no scheme
/// — the caller falls back to the full URL string, which is the right
/// shape for non-http URIs (e.g. `file://...`, custom schemes).
fn extract_domain(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let host = after_scheme.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    /// A small theme with a distinctive `link` colour so the assertions
    /// can verify the spans are styled correctly.
    fn test_theme() -> Theme {
        Theme::hearth()
    }

    /// Collapse a `Line` into its concatenated content for substring
    /// assertions — terse helper used by every render test below.
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn sources_block_renders_only_when_urls_present() {
        let theme = test_theme();
        let lines = render_sources(&[], &theme);
        assert!(
            lines.is_empty(),
            "an empty URL list must produce zero lines (no stray header)"
        );

        let one = vec!["https://example.com".to_string()];
        let lines = render_sources(&one, &theme);
        // Header + 1 entry.
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "Sources:");
    }

    #[test]
    fn sources_max_10_with_overflow_dropped() {
        let theme = test_theme();
        // 15 URLs — the block must cap at 10 entries after the header.
        let urls: Vec<String> = (0..15).map(|i| format!("https://r{i}.com")).collect();
        let lines = render_sources(&urls, &theme);
        // 1 header + 10 entries.
        assert_eq!(lines.len(), 11, "must cap at 10 entries + 1 header");
        // The 11th URL (index 10) must NOT appear anywhere.
        let body: String = lines
            .iter()
            .skip(1)
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains("r9.com"), "10th URL must be present");
        assert!(
            !body.contains("r10.com"),
            "11th URL must be silently dropped"
        );
    }

    #[test]
    fn domain_extracted_from_url() {
        let theme = test_theme();
        let urls = vec!["https://example.com/path?q=1".to_string()];
        let lines = render_sources(&urls, &theme);
        // Row 1 is the entry.
        let row = line_text(&lines[1]);
        assert!(
            row.contains("example.com"),
            "entry must surface the domain — got: {row}"
        );
        assert!(
            row.contains("https://example.com/path?q=1"),
            "entry must keep the full URL inline — got: {row}"
        );
        assert!(!row.contains("path?q=1 — "), "domain must not include path");
    }

    #[test]
    fn sources_use_theme_link_color() {
        // Every span (header + entries) must paint with `theme.link` so
        // the Sources block reads as the canonical "link" affordance and
        // not as body text.
        let theme = test_theme();
        let urls = vec!["https://a.com".to_string(), "https://b.com".to_string()];
        let lines = render_sources(&urls, &theme);
        for line in &lines {
            for span in &line.spans {
                assert_eq!(
                    span.style.fg,
                    Some(theme.link),
                    "every Sources span must carry theme.link as its fg colour"
                );
            }
        }
    }

    #[test]
    fn non_http_url_uses_full_url_as_domain_fallback() {
        // A URL without a `://` scheme (rare, but feasible for engine-
        // emitted file paths or custom URI shapes) has no extractable
        // host. The renderer falls back to using the URL string in the
        // domain slot, so the row still reads as `1. <url> — <url>` —
        // ugly but never empty.
        let theme = test_theme();
        let urls = vec!["bare-token-no-scheme".to_string()];
        let lines = render_sources(&urls, &theme);
        let row = line_text(&lines[1]);
        assert!(
            row.contains("bare-token-no-scheme"),
            "fallback domain must be the input string — got: {row}"
        );
    }

    #[test]
    fn empty_host_fragment_falls_back_to_full_url() {
        // An input like `https:///path` (technically malformed) parses
        // to an empty host fragment; the renderer must fall back to the
        // full URL rather than print an empty domain.
        let theme = test_theme();
        let urls = vec!["https:///orphan".to_string()];
        let lines = render_sources(&urls, &theme);
        let row = line_text(&lines[1]);
        assert!(
            row.contains("https:///orphan"),
            "malformed URL must fall back to the full string — got: {row}"
        );
    }

    #[test]
    fn header_styled_in_link_color() {
        // A regression guard for the header style — the `Sources:`
        // header must carry the link colour, not the default fg.
        let theme = test_theme();
        let urls = vec!["https://x.com".to_string()];
        let lines = render_sources(&urls, &theme);
        let header = &lines[0];
        assert_eq!(line_text(header), "Sources:");
        // Header has one span; it must be link-coloured.
        assert_eq!(header.spans.len(), 1);
        assert_eq!(header.spans[0].style.fg, Some(theme.link));
    }

    #[test]
    fn sources_url_is_osc8_wrapped_v092() {
        // v0.9.2 W9 (S24): each http(s) URL entry carries the OSC 8
        // opener + closer so it is clickable.
        let theme = test_theme();
        let urls = vec!["https://example.com/path".to_string()];
        let lines = render_sources(&urls, &theme);
        let row = &lines[1];
        let has_open = row
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;https://example.com/path\x07");
        let has_close = row
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "\x1b]8;;\x07");
        assert!(
            has_open,
            "sources URL missing OSC 8 opener; spans: {:?}",
            row.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<Vec<_>>()
        );
        assert!(has_close, "sources URL missing OSC 8 closer");
        // The visible URL text is still its own span (zero-width escapes).
        assert!(
            row.spans
                .iter()
                .any(|s| s.content.as_ref() == "https://example.com/path")
        );
    }

    #[test]
    fn sources_mailto_url_renders_plain_v092() {
        // A mailto: source is rendered plain — no OSC 8 escape.
        let theme = test_theme();
        let urls = vec!["mailto:team@example.com".to_string()];
        let lines = render_sources(&urls, &theme);
        let row = &lines[1];
        let any_osc8 = row.spans.iter().any(|s| s.content.contains("\x1b]8;;"));
        assert!(!any_osc8, "mailto source must not emit OSC 8 escape");
        let joined = line_text(row);
        assert!(joined.contains("mailto:team@example.com"));
    }

    #[test]
    fn no_color_theme_uses_reset_color() {
        // With `NO_COLOR`, every span resolves to `Color::Reset` so the
        // Sources block paints monochrome alongside the rest of the
        // transcript (no stray theme leak).
        let theme = Theme::no_color();
        let urls = vec!["https://x.com".to_string()];
        let lines = render_sources(&urls, &theme);
        for line in &lines {
            for span in &line.spans {
                assert_eq!(span.style.fg, Some(Color::Reset));
            }
        }
    }
}
