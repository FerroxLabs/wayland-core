//! Readability-style markdown extraction for `BrowserOp::Read`.
//!
//! Simplified Readability heuristic:
//!   1. Strip `<nav>`, `<header>`, `<footer>`, `<aside>`, `<script>`, `<style>`,
//!      `<form>`, `<noscript>` blocks.
//!   2. Find the highest-scoring `<article>` / `<main>` / content-dense `<div>`.
//!   3. Walk surviving DOM, emit markdown via tag-to-md visitor.
//!
//! No dependency on an HTML parser crate at this layer — input is the raw HTML
//! string; we use a small token-based scanner. For production, swap this
//! function out for `html5ever`-driven extraction; the public API stays.

use crate::op::ReadMode;

/// Extract main-content markdown from raw HTML. Returns trimmed markdown
/// (<= ~few KB on a typical news article). On heuristic failure, returns
/// the whole-page text fall-back (Raw-mode behaviour).
pub fn extract(html: &str, mode: ReadMode) -> String {
    let stripped = strip_chrome(html);
    let body = match mode {
        ReadMode::Raw => stripped.clone(),
        ReadMode::Article | ReadMode::MainContent => extract_main(&stripped).unwrap_or(stripped),
    };
    to_markdown(&body)
}

/// Drop block-level chrome elements + scripts/styles. Greedy outer-most match
/// per tag so nested instances also disappear.
fn strip_chrome(html: &str) -> String {
    let mut s = html.to_string();
    for tag in [
        "script", "style", "noscript", "nav", "header", "footer", "aside", "form",
    ] {
        s = strip_block(&s, tag);
    }
    s
}

fn strip_block(input: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    let lower = input.to_ascii_lowercase();
    while let Some(rel) = lower[cursor..].find(&open) {
        let start = cursor + rel;
        // Make sure this is a tag (next char must be > or whitespace).
        let after = start + open.len();
        if after < input.len() {
            let ch = input.as_bytes()[after];
            if ch != b'>' && !ch.is_ascii_whitespace() && ch != b'/' {
                out.push_str(&input[cursor..start + 1]);
                cursor = start + 1;
                continue;
            }
        }
        out.push_str(&input[cursor..start]);
        if let Some(close_rel) = lower[start..].find(&close) {
            cursor = start + close_rel + close.len();
        } else {
            // Unclosed — drop to end.
            cursor = input.len();
        }
    }
    out.push_str(&input[cursor..]);
    out
}

/// Find the highest-density content block by counting text-bearing tags
/// inside `<article>`, `<main>`, or `<div>` with content-related class hints.
fn extract_main(html: &str) -> Option<String> {
    for tag in ["article", "main"] {
        if let Some(content) = first_block(html, tag) {
            return Some(content);
        }
    }
    // Fallback: pick the longest `<div>...</div>` whose inner text exceeds
    // a basic density threshold. Cheap heuristic — production version
    // would replace with `html5ever` + Readability proper.
    let mut best: Option<(usize, String)> = None;
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(rel) = lower[cursor..].find("<div") {
        let start = cursor + rel;
        // Skip if not a real <div tag.
        let after = start + 4;
        if after >= html.len()
            || (html.as_bytes()[after] != b'>'
                && !html.as_bytes()[after].is_ascii_whitespace()
                && html.as_bytes()[after] != b'/')
        {
            cursor = after;
            continue;
        }
        let close = lower[start..].find("</div>");
        if let Some(close_rel) = close {
            let block = &html[start..start + close_rel + "</div>".len()];
            let text_len = visible_text_len(block);
            if text_len > 200 && best.as_ref().map(|(n, _)| text_len > *n).unwrap_or(true) {
                best = Some((text_len, block.to_string()));
            }
            cursor = start + close_rel + "</div>".len();
        } else {
            break;
        }
    }
    best.map(|(_, s)| s)
}

fn first_block(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = lower.find(&open)?;
    // Ensure the next char is > or whitespace.
    let after = start + open.len();
    if after >= html.len() {
        return None;
    }
    let ch = html.as_bytes()[after];
    if ch != b'>' && !ch.is_ascii_whitespace() && ch != b'/' {
        return None;
    }
    let close_rel = lower[start..].find(&close)?;
    Some(html[start..start + close_rel + close.len()].to_string())
}

fn visible_text_len(block: &str) -> usize {
    // Strip tags entirely; count remaining whitespace-collapsed chars.
    let mut in_tag = false;
    let mut count = 0usize;
    for c in block.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag && !c.is_whitespace() => count += 1,
            _ => {}
        }
    }
    count
}

/// Tag-to-md visitor. Maps common block + inline tags to markdown; otherwise
/// drops tags and emits inner text.
fn to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut paragraph_open = false;

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            tag_buf.clear();
            continue;
        }
        if c == '>' {
            in_tag = false;
            let tl = tag_buf.to_ascii_lowercase();
            let tl_trim = tl.trim_start_matches('/');
            let closing = tl.starts_with('/');
            let tag_name = tl_trim
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('/');
            match tag_name {
                "h1" if !closing => out.push_str("\n# "),
                "h2" if !closing => out.push_str("\n## "),
                "h3" if !closing => out.push_str("\n### "),
                "h4" if !closing => out.push_str("\n#### "),
                "h5" if !closing => out.push_str("\n##### "),
                "h6" if !closing => out.push_str("\n###### "),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if closing => out.push('\n'),
                "p" if !closing => {
                    out.push_str("\n\n");
                    paragraph_open = true;
                }
                "p" if closing && paragraph_open => {
                    paragraph_open = false;
                }
                "br" => out.push('\n'),
                "li" if !closing => out.push_str("\n- "),
                "strong" | "b" if !closing => out.push_str("**"),
                "strong" | "b" if closing => out.push_str("**"),
                "em" | "i" if !closing => out.push('*'),
                "em" | "i" if closing => out.push('*'),
                "code" if !closing => out.push('`'),
                "code" if closing => out.push('`'),
                _ => {}
            }
            tag_buf.clear();
            continue;
        }
        if in_tag {
            tag_buf.push(c);
        } else {
            out.push(c);
        }
    }

    // Collapse repeated whitespace, decode common entities.
    let mut squeezed = String::with_capacity(out.len());
    let mut prev_blank = false;
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                squeezed.push('\n');
            }
            prev_blank = true;
        } else {
            squeezed.push_str(trimmed);
            squeezed.push('\n');
            prev_blank = false;
        }
    }
    squeezed = squeezed
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    squeezed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEWS_FIXTURE: &str = r#"
<!doctype html>
<html><head><title>News Site</title><style>.x{}</style></head>
<body>
<nav><a>Home</a><a>About</a></nav>
<header><h1>Site Title</h1></header>
<main>
  <article>
    <h1>Breaking: Important Headline Here</h1>
    <p>This is the first paragraph of the article body. It contains
    interesting information about a current event that readers should
    know about.</p>
    <p>Second paragraph continues the story with more details and
    quotes from <em>relevant</em> sources.</p>
    <p>Third paragraph wraps things up with a concluding observation.</p>
  </article>
</main>
<aside><div class="ads">Buy now</div></aside>
<footer>(c) NewsCorp</footer>
<script>console.log('tracking')</script>
</body></html>
"#;

    #[test]
    fn extract_main_content_drops_nav_aside_footer() {
        let md = extract(NEWS_FIXTURE, ReadMode::MainContent);
        assert!(md.contains("Breaking: Important Headline Here"));
        assert!(md.contains("first paragraph"));
        assert!(md.contains("Second paragraph"));
        assert!(
            !md.contains("Buy now"),
            "aside content must be stripped from md output"
        );
        assert!(
            !md.contains("(c) NewsCorp"),
            "footer must be stripped from md output"
        );
        assert!(
            !md.contains("Home") || !md.contains("About"),
            "nav links must be stripped"
        );
    }

    #[test]
    fn extracted_markdown_is_compact() {
        let md = extract(NEWS_FIXTURE, ReadMode::MainContent);
        assert!(
            md.len() < 2048,
            "expected <2KB compact markdown, got {} bytes",
            md.len()
        );
    }

    #[test]
    fn raw_mode_includes_everything_after_chrome_strip() {
        let md = extract(NEWS_FIXTURE, ReadMode::Raw);
        // Raw mode still strips chrome (nav/aside/footer/script).
        assert!(md.contains("Breaking"));
        assert!(!md.contains("Buy now"));
    }

    #[test]
    fn handles_html_without_article_or_main() {
        let html = "<html><body><div>Hello <b>world</b></div></body></html>";
        let md = extract(html, ReadMode::MainContent);
        // Falls back to raw + tag-strip behaviour.
        assert!(md.contains("Hello"));
        assert!(md.contains("**world**"));
    }
}
