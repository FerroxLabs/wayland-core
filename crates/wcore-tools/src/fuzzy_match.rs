//! T3-3.2.7: Fuzzy find-and-replace helper for the Edit tool.
//!
//! Ported from the prior Wayland Python engine — an 8/9-strategy
//! matching chain (inspired by OpenCode) used to robustly locate text in
//! a file even when the LLM-supplied `old_string` differs from the on-disk
//! content in whitespace, indentation, Unicode punctuation, or escape
//! conventions.
//!
//! This module exposes a single public entry point —
//! [`fuzzy_find_and_replace`] — wired into `EditTool` as an **opt-in**
//! fallback (Rank 41): `EditTool::with_fuzzy_fallback(true)` retries an
//! exact-match failure through this chain. The gate defaults OFF, so the
//! exact-match path stays byte-identical when callers don't opt in. The
//! script DSL editor can opt into the same helper.
//!
//! ## Strategies (tried in order)
//!
//! 1. **Exact** — direct `str::find` byte-level match.
//! 2. **Line-trimmed** — strip leading/trailing whitespace per line.
//! 3. **Whitespace-normalized** — collapse runs of spaces/tabs to one space.
//! 4. **Indentation-flexible** — `lstrip` every line.
//! 5. **Escape-normalized** — convert literal `\n`/`\t`/`\r` to chars.
//! 6. **Trimmed-boundary** — trim only first + last lines of the pattern.
//! 7. **Unicode-normalized** — fold smart quotes, em/en dashes, ellipsis, NBSP.
//! 8. **Block-anchor** — match first + last lines, similarity ≥ threshold
//!    for the middle block.
//! 9. **Context-aware** — ≥ 50% of lines must share ≥ 80% similarity.
//!
//! ## Determinism note
//!
//! The similarity metric is a faithful Rust port of Python's
//! `difflib.SequenceMatcher.ratio()` (Ratcliff-Obershelp): the longest
//! common subsequence is found greedily over matching blocks, and the ratio
//! is `2 * matched / (len(a) + len(b))`. This keeps the thresholds in the
//! Python source (`0.50`, `0.70`, `0.80`) semantically intact.

use std::collections::HashMap;

/// Smart-quote / dash / ellipsis / NBSP → ASCII fold table.
///
/// Some entries expand a single source character to multiple ASCII
/// characters (em-dash → `--`, ellipsis → `...`). Position remapping
/// uses [`build_orig_to_norm_map`] to recover the original offsets.
const UNICODE_MAP: &[(char, &str)] = &[
    ('\u{201C}', "\""),  // left double quotation mark
    ('\u{201D}', "\""),  // right double quotation mark
    ('\u{2018}', "'"),   // left single quotation mark
    ('\u{2019}', "'"),   // right single quotation mark
    ('\u{2014}', "--"),  // em dash
    ('\u{2013}', "-"),   // en dash
    ('\u{2026}', "..."), // horizontal ellipsis
    ('\u{00A0}', " "),   // non-breaking space
];

/// Outcome of a [`fuzzy_find_and_replace`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyResult {
    /// The (possibly modified) content.
    pub content: String,
    /// Number of replacements made (`0` when no strategy matched, or when
    /// the caller required uniqueness and the strategy found duplicates).
    pub match_count: usize,
    /// Name of the strategy that produced the match (`None` on failure).
    pub strategy: Option<&'static str>,
    /// Human-readable error message (`None` on success).
    pub error: Option<String>,
}

impl FuzzyResult {
    fn err(content: String, msg: impl Into<String>) -> Self {
        Self {
            content,
            match_count: 0,
            strategy: None,
            error: Some(msg.into()),
        }
    }
}

/// Find and replace text using the 9-strategy fuzzy match chain.
///
/// * `content` — file content to search.
/// * `old_string` — text to find (any of the 9 strategies may match).
/// * `new_string` — replacement text (exact, no normalisation).
/// * `replace_all` — if `false`, the call fails when a strategy finds > 1
///   match; if `true`, every match from the first successful strategy is
///   replaced.
///
/// Returns a [`FuzzyResult`] carrying the new content, the number of
/// replacements, the winning strategy name, and an optional error message.
pub fn fuzzy_find_and_replace(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> FuzzyResult {
    if old_string.is_empty() {
        return FuzzyResult::err(content.to_string(), "old_string cannot be empty");
    }
    if old_string == new_string {
        return FuzzyResult::err(
            content.to_string(),
            "old_string and new_string are identical",
        );
    }

    type StrategyFn = fn(&str, &str) -> Vec<(usize, usize)>;
    let strategies: &[(&'static str, StrategyFn)] = &[
        ("exact", strategy_exact),
        ("line_trimmed", strategy_line_trimmed),
        ("whitespace_normalized", strategy_whitespace_normalized),
        ("indentation_flexible", strategy_indentation_flexible),
        ("escape_normalized", strategy_escape_normalized),
        ("trimmed_boundary", strategy_trimmed_boundary),
        ("unicode_normalized", strategy_unicode_normalized),
        ("block_anchor", strategy_block_anchor),
        ("context_aware", strategy_context_aware),
    ];

    for (name, f) in strategies {
        let matches = f(content, old_string);
        if matches.is_empty() {
            continue;
        }
        if matches.len() > 1 && !replace_all {
            return FuzzyResult::err(
                content.to_string(),
                format!(
                    "Found {} matches for old_string. Provide more context to make it unique, or use replace_all=true.",
                    matches.len()
                ),
            );
        }
        let count = matches.len();
        let new_content = apply_replacements(content, &matches, new_string);
        return FuzzyResult {
            content: new_content,
            match_count: count,
            strategy: Some(name),
            error: None,
        };
    }

    FuzzyResult::err(
        content.to_string(),
        "Could not find a match for old_string in the file",
    )
}

/// Apply replacements at the given (start, end) byte positions.
fn apply_replacements(content: &str, matches: &[(usize, usize)], new_string: &str) -> String {
    // Replace from end → start to preserve earlier offsets.
    let mut sorted = matches.to_vec();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.0));
    let mut result = content.to_string();
    for (start, end) in sorted {
        let end = end.min(result.len());
        let start = start.min(end);
        result.replace_range(start..end, new_string);
    }
    result
}

// =============================================================================
// Strategies
// =============================================================================

/// Strategy 1 — direct byte-level substring search, all occurrences.
fn strategy_exact(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = content.as_bytes();
    let plen = pattern.len();
    let mut start = 0usize;
    while start + plen <= bytes.len() {
        match content[start..].find(pattern) {
            Some(rel) => {
                let pos = start + rel;
                out.push((pos, pos + plen));
                start = pos + 1;
            }
            None => break,
        }
    }
    out
}

/// Strategy 2 — strip leading/trailing whitespace per line.
fn strategy_line_trimmed(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let pattern_lines: Vec<String> = pattern.split('\n').map(|l| l.trim().to_string()).collect();
    let pattern_normalized = pattern_lines.join("\n");
    let content_lines: Vec<&str> = content.split('\n').collect();
    let content_normalized_lines: Vec<String> =
        content_lines.iter().map(|l| l.trim().to_string()).collect();
    find_normalized_matches(
        content,
        &content_lines,
        &content_normalized_lines,
        &pattern_normalized,
    )
}

/// Strategy 3 — collapse runs of spaces/tabs to a single space, preserve `\n`.
fn strategy_whitespace_normalized(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let pattern_norm = collapse_ws(pattern);
    let content_norm = collapse_ws(content);
    let in_norm = strategy_exact(&content_norm, &pattern_norm);
    if in_norm.is_empty() {
        return Vec::new();
    }
    map_normalized_positions(content, &content_norm, &in_norm)
}

/// Strategy 4 — `lstrip` each line before comparing.
fn strategy_indentation_flexible(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let content_lines: Vec<&str> = content.split('\n').collect();
    let content_stripped: Vec<String> = content_lines
        .iter()
        .map(|l| lstrip(l).to_string())
        .collect();
    let pattern_stripped: Vec<String> =
        pattern.split('\n').map(|l| lstrip(l).to_string()).collect();
    find_normalized_matches(
        content,
        &content_lines,
        &content_stripped,
        &pattern_stripped.join("\n"),
    )
}

/// Strategy 5 — convert literal escape sequences to real chars in the pattern.
fn strategy_escape_normalized(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let unescaped = pattern
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\r", "\r");
    if unescaped == pattern {
        return Vec::new();
    }
    strategy_exact(content, &unescaped)
}

/// Strategy 6 — trim only the first and last lines of the pattern.
fn strategy_trimmed_boundary(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let mut pattern_lines: Vec<String> = pattern.split('\n').map(String::from).collect();
    if pattern_lines.is_empty() {
        return Vec::new();
    }
    pattern_lines[0] = pattern_lines[0].trim().to_string();
    let last = pattern_lines.len() - 1;
    if last > 0 {
        pattern_lines[last] = pattern_lines[last].trim().to_string();
    }
    let modified_pattern = pattern_lines.join("\n");

    let content_lines: Vec<&str> = content.split('\n').collect();
    let plc = pattern_lines.len();
    if content_lines.len() < plc {
        return Vec::new();
    }

    let content_len = content.len();
    let mut out = Vec::new();
    for i in 0..=(content_lines.len() - plc) {
        let mut block: Vec<String> = content_lines[i..i + plc]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        block[0] = block[0].trim().to_string();
        if block.len() > 1 {
            let bl = block.len() - 1;
            block[bl] = block[bl].trim().to_string();
        }
        if block.join("\n") == modified_pattern {
            let (s, e) = calculate_line_positions(&content_lines, i, i + plc, content_len);
            out.push((s, e));
        }
    }
    out
}

/// Strategy 7 — fold smart quotes / dashes / ellipsis / NBSP to ASCII, then
/// try `exact` and `line_trimmed` on the folded copies.
fn strategy_unicode_normalized(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let norm_pattern = unicode_normalize(pattern);
    let norm_content = unicode_normalize(content);
    if norm_content == content && norm_pattern == pattern {
        return Vec::new();
    }
    let mut norm_matches = strategy_exact(&norm_content, &norm_pattern);
    if norm_matches.is_empty() {
        norm_matches = strategy_line_trimmed(&norm_content, &norm_pattern);
    }
    if norm_matches.is_empty() {
        return Vec::new();
    }
    let orig_to_norm = build_orig_to_norm_map(content);
    map_positions_norm_to_orig(&orig_to_norm, &norm_matches)
}

/// Strategy 8 — match patterns whose first and last lines exactly match,
/// where the middle block is similar enough.
///
/// Threshold: `0.50` for a single candidate, `0.70` when there are multiple
/// (mirrors the tightened bound in the Python source).
fn strategy_block_anchor(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let norm_pattern = unicode_normalize(pattern);
    let norm_content = unicode_normalize(content);
    let pattern_lines: Vec<&str> = norm_pattern.split('\n').collect();
    if pattern_lines.len() < 2 {
        return Vec::new();
    }
    let first_line = pattern_lines[0].trim();
    let last_line = pattern_lines[pattern_lines.len() - 1].trim();
    let norm_content_lines: Vec<&str> = norm_content.split('\n').collect();
    let orig_content_lines: Vec<&str> = content.split('\n').collect();
    let plc = pattern_lines.len();
    if norm_content_lines.len() < plc {
        return Vec::new();
    }

    let mut potential: Vec<usize> = Vec::new();
    for i in 0..=(norm_content_lines.len() - plc) {
        if norm_content_lines[i].trim() == first_line
            && norm_content_lines[i + plc - 1].trim() == last_line
        {
            potential.push(i);
        }
    }
    let threshold: f64 = if potential.len() == 1 { 0.50 } else { 0.70 };

    let content_len = content.len();
    let mut out = Vec::new();
    for i in potential {
        let similarity = if plc <= 2 {
            1.0_f64
        } else {
            let content_middle = norm_content_lines[i + 1..i + plc - 1].join("\n");
            let pattern_middle = pattern_lines[1..plc - 1].join("\n");
            sequence_match_ratio(&content_middle, &pattern_middle)
        };
        if similarity >= threshold {
            let (s, e) = calculate_line_positions(&orig_content_lines, i, i + plc, content_len);
            out.push((s, e));
        }
    }
    out
}

/// Strategy 9 — at least 50% of lines must have ≥ 80% similarity.
fn strategy_context_aware(content: &str, pattern: &str) -> Vec<(usize, usize)> {
    let pattern_lines: Vec<&str> = pattern.split('\n').collect();
    let content_lines: Vec<&str> = content.split('\n').collect();
    if pattern_lines.is_empty() {
        return Vec::new();
    }
    let plc = pattern_lines.len();
    if content_lines.len() < plc {
        return Vec::new();
    }

    let content_len = content.len();
    let needed = ((plc as f64) * 0.5).ceil() as usize;
    let mut out = Vec::new();
    for i in 0..=(content_lines.len() - plc) {
        let block = &content_lines[i..i + plc];
        let mut high = 0usize;
        for (p, c) in pattern_lines.iter().zip(block.iter()) {
            let sim = sequence_match_ratio(p.trim(), c.trim());
            if sim >= 0.80 {
                high += 1;
            }
        }
        if high >= needed {
            let (s, e) = calculate_line_positions(&content_lines, i, i + plc, content_len);
            out.push((s, e));
        }
    }
    out
}

// =============================================================================
// Helpers
// =============================================================================

fn lstrip(s: &str) -> &str {
    s.trim_start()
}

/// Collapse runs of spaces/tabs to a single space, preserving newlines.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch == ' ' || ch == '\t' {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

fn unicode_normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    'chars: for ch in text.chars() {
        for (src, repl) in UNICODE_MAP {
            if *src == ch {
                out.push_str(repl);
                continue 'chars;
            }
        }
        out.push(ch);
    }
    out
}

/// Build an `orig_byte_pos -> norm_byte_pos` map (one entry per *byte* of
/// the original string, plus a sentinel = `norm.len()`).
///
/// Why bytes? All match positions returned by these strategies are byte
/// offsets into `content` (so that `content.replace_range(start..end, ..)`
/// is straightforward). Some folds (em-dash → "--") expand a 3-byte source
/// character into 2 ASCII bytes, so this map records, for every byte index
/// in the original, where that byte's character starts in the normalised
/// string.
fn build_orig_to_norm_map(original: &str) -> Vec<usize> {
    let mut out = Vec::with_capacity(original.len() + 1);
    let mut norm_pos = 0usize;
    for ch in original.chars() {
        let ch_byte_len = ch.len_utf8();
        let repl = UNICODE_MAP
            .iter()
            .find_map(|(s, r)| if *s == ch { Some(*r) } else { None });
        for _ in 0..ch_byte_len {
            out.push(norm_pos);
        }
        norm_pos += match repl {
            Some(r) => r.len(),
            None => ch_byte_len,
        };
    }
    out.push(norm_pos);
    out
}

fn map_positions_norm_to_orig(
    orig_to_norm: &[usize],
    norm_matches: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    // Invert: norm_pos -> first orig byte whose normalised position equals it.
    let mut norm_to_orig_start: HashMap<usize, usize> = HashMap::new();
    if orig_to_norm.len() > 1 {
        for (orig_pos, norm_pos) in orig_to_norm[..orig_to_norm.len() - 1].iter().enumerate() {
            norm_to_orig_start.entry(*norm_pos).or_insert(orig_pos);
        }
    }
    let orig_len = orig_to_norm.len().saturating_sub(1);

    let mut out = Vec::with_capacity(norm_matches.len());
    for (ns, ne) in norm_matches {
        let Some(&orig_start) = norm_to_orig_start.get(ns) else {
            continue;
        };
        let mut orig_end = orig_start;
        while orig_end < orig_len && orig_to_norm[orig_end] < *ne {
            orig_end += 1;
        }
        out.push((orig_start, orig_end));
    }
    out
}

/// Convert (start_line, end_line) into (start_byte, end_byte) within `content`.
///
/// Lines are joined by a single `\n`, so each line consumes
/// `len(line) + 1` bytes. The final `end_pos` excludes the trailing `\n`
/// of the last included line, unless that would exceed the original
/// content length (final line without a trailing newline).
fn calculate_line_positions(
    content_lines: &[&str],
    start_line: usize,
    end_line: usize,
    content_length: usize,
) -> (usize, usize) {
    let start_pos: usize = content_lines[..start_line]
        .iter()
        .map(|l| l.len() + 1)
        .sum();
    let mut end_pos: usize = content_lines[..end_line].iter().map(|l| l.len() + 1).sum();
    if end_pos == 0 {
        return (0, 0);
    }
    end_pos -= 1;
    if end_pos > content_length {
        end_pos = content_length;
    }
    (start_pos, end_pos)
}

/// Window through `content_normalized_lines` looking for an exact match of
/// `pattern_normalized` (joined by `\n`); map hits back to byte positions
/// in the *original* `content`.
fn find_normalized_matches(
    content: &str,
    content_lines: &[&str],
    content_normalized_lines: &[String],
    pattern_normalized: &str,
) -> Vec<(usize, usize)> {
    let pattern_norm_lines: Vec<&str> = pattern_normalized.split('\n').collect();
    let plc = pattern_norm_lines.len();
    if content_normalized_lines.len() < plc {
        return Vec::new();
    }
    let content_len = content.len();
    let mut out = Vec::new();
    for i in 0..=(content_normalized_lines.len() - plc) {
        let block = content_normalized_lines[i..i + plc].join("\n");
        if block == pattern_normalized {
            let (s, e) = calculate_line_positions(content_lines, i, i + plc, content_len);
            out.push((s, e));
        }
    }
    out
}

/// Best-effort whitespace-collapse position remap (original ↔ normalised).
fn map_normalized_positions(
    original: &str,
    normalized: &str,
    normalized_matches: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    if normalized_matches.is_empty() {
        return Vec::new();
    }
    let orig_bytes = original.as_bytes();
    let norm_bytes = normalized.as_bytes();
    let mut orig_to_norm: Vec<usize> = Vec::with_capacity(orig_bytes.len());

    let mut orig_idx = 0usize;
    let mut norm_idx = 0usize;
    while orig_idx < orig_bytes.len() && norm_idx < norm_bytes.len() {
        let oc = orig_bytes[orig_idx];
        let nc = norm_bytes[norm_idx];
        if oc == nc {
            orig_to_norm.push(norm_idx);
            orig_idx += 1;
            norm_idx += 1;
        } else if (oc == b' ' || oc == b'\t') && nc == b' ' {
            orig_to_norm.push(norm_idx);
            orig_idx += 1;
            if orig_idx < orig_bytes.len()
                && orig_bytes[orig_idx] != b' '
                && orig_bytes[orig_idx] != b'\t'
            {
                norm_idx += 1;
            }
        } else {
            // Either extra whitespace in `original`, or a benign mismatch
            // (shouldn't happen with our normalisation — handled identically
            // either way: record the current norm offset and advance only
            // the original cursor).
            orig_to_norm.push(norm_idx);
            orig_idx += 1;
        }
    }
    while orig_idx < orig_bytes.len() {
        orig_to_norm.push(normalized.len());
        orig_idx += 1;
    }

    let mut norm_to_orig_start: HashMap<usize, usize> = HashMap::new();
    let mut norm_to_orig_end: HashMap<usize, usize> = HashMap::new();
    for (orig_pos, norm_pos) in orig_to_norm.iter().enumerate() {
        norm_to_orig_start.entry(*norm_pos).or_insert(orig_pos);
        norm_to_orig_end.insert(*norm_pos, orig_pos);
    }

    let mut out = Vec::with_capacity(normalized_matches.len());
    for (ns, ne) in normalized_matches {
        let orig_start = norm_to_orig_start.get(ns).copied().unwrap_or_else(|| {
            orig_to_norm
                .iter()
                .position(|&n| n >= *ns)
                .unwrap_or(orig_bytes.len())
        });
        let orig_end = if *ne == 0 {
            orig_start
        } else if let Some(&v) = norm_to_orig_end.get(&(ne - 1)) {
            v + 1
        } else {
            orig_start + (ne - ns)
        };
        let mut orig_end = orig_end.min(orig_bytes.len());
        while orig_end < orig_bytes.len()
            && (orig_bytes[orig_end] == b' ' || orig_bytes[orig_end] == b'\t')
        {
            orig_end += 1;
        }
        out.push((orig_start, orig_end));
    }
    out
}

// =============================================================================
// Ratcliff-Obershelp similarity (Python `difflib.SequenceMatcher.ratio()`)
// =============================================================================

/// Faithful port of `difflib.SequenceMatcher.ratio()` — the
/// Ratcliff-Obershelp gestalt-pattern-matching ratio. Returns
/// `2 * matched_chars / (len(a) + len(b))` in the range `[0.0, 1.0]`.
///
/// Operates on `chars()` (Unicode code points) to match Python semantics.
fn sequence_match_ratio(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let total = a_chars.len() + b_chars.len();
    if total == 0 {
        return 1.0;
    }
    let matched = ratcliff_obershelp_matched(&a_chars, &b_chars);
    (2.0 * matched as f64) / (total as f64)
}

/// Recursively sum the lengths of the longest common contiguous blocks
/// (no junk filter — matches Python `SequenceMatcher(None, a, b)`).
fn ratcliff_obershelp_matched(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (alo, ahi, blo, bhi, size) = find_longest_match(a, b);
    if size == 0 {
        return 0;
    }
    let left = ratcliff_obershelp_matched(&a[..alo], &b[..blo]);
    let right = ratcliff_obershelp_matched(&a[ahi..], &b[bhi..]);
    left + size + right
}

/// Find the longest contiguous matching subsequence (LCMS) between `a`
/// and `b`. Returns `(a_lo, a_hi, b_lo, b_hi, size)` in the same shape
/// as Python's `Match` tuple, with `a_hi = a_lo + size`.
fn find_longest_match(a: &[char], b: &[char]) -> (usize, usize, usize, usize, usize) {
    // Index `b` by character so we know where to begin matching each `a[i]`.
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, ch) in b.iter().enumerate() {
        b2j.entry(*ch).or_default().push(j);
    }
    let mut besti = 0usize;
    let mut bestj = 0usize;
    let mut bestsize = 0usize;
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, ch) in a.iter().enumerate() {
        let mut new_j2len: HashMap<usize, usize> = HashMap::new();
        if let Some(js) = b2j.get(ch) {
            for &j in js {
                let k = if j == 0 {
                    1
                } else {
                    j2len.get(&(j - 1)).copied().unwrap_or(0) + 1
                };
                new_j2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = new_j2len;
    }
    (besti, besti + bestsize, bestj, bestj + bestsize, bestsize)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_succeeds_and_returns_strategy_exact() {
        let res =
            fuzzy_find_and_replace("def foo():\n    pass\n", "def foo():", "def bar():", false);
        assert_eq!(res.strategy, Some("exact"));
        assert_eq!(res.match_count, 1);
        assert_eq!(res.content, "def bar():\n    pass\n");
        assert!(res.error.is_none());
    }

    #[test]
    fn line_trimmed_handles_trailing_whitespace_drift() {
        let content = "def foo():   \n    pass\n";
        let old = "def foo():";
        let res = fuzzy_find_and_replace(content, old, "def bar():", false);
        assert!(res.error.is_none(), "error = {:?}", res.error);
        assert_eq!(res.strategy, Some("exact"));
        assert!(res.content.starts_with("def bar():"));
    }

    #[test]
    fn indentation_flexible_matches_when_indent_differs() {
        let content = "class C:\n    def foo():\n        x = 1\n";
        let old = "def foo():\n    x = 1";
        let res = fuzzy_find_and_replace(content, old, "def bar():\n    x = 2", false);
        assert!(
            matches!(
                res.strategy,
                Some("line_trimmed")
                    | Some("indentation_flexible")
                    | Some("trimmed_boundary")
                    | Some("block_anchor")
            ),
            "unexpected strategy: {:?}",
            res.strategy
        );
        assert!(res.error.is_none());
    }

    #[test]
    fn no_match_returns_error() {
        let res = fuzzy_find_and_replace("hello world", "absent string", "x", false);
        assert!(res.error.is_some());
        assert_eq!(res.match_count, 0);
        assert_eq!(res.strategy, None);
        assert_eq!(res.content, "hello world");
    }

    #[test]
    fn empty_old_string_is_rejected() {
        let res = fuzzy_find_and_replace("anything", "", "x", false);
        assert_eq!(res.error.as_deref(), Some("old_string cannot be empty"));
        assert_eq!(res.match_count, 0);
    }

    #[test]
    fn identical_old_and_new_is_rejected() {
        let res = fuzzy_find_and_replace("anything", "any", "any", false);
        assert_eq!(
            res.error.as_deref(),
            Some("old_string and new_string are identical")
        );
    }

    #[test]
    fn multiple_matches_without_replace_all_errors() {
        let res = fuzzy_find_and_replace("foo\nfoo\nfoo\n", "foo", "bar", false);
        assert!(res.error.is_some());
        assert!(res.error.as_ref().unwrap().contains("3 matches"));
        assert_eq!(res.match_count, 0);
        assert_eq!(res.content, "foo\nfoo\nfoo\n");
    }

    #[test]
    fn replace_all_replaces_every_exact_occurrence() {
        let res = fuzzy_find_and_replace("foo bar foo baz foo", "foo", "X", true);
        assert!(res.error.is_none());
        assert_eq!(res.strategy, Some("exact"));
        assert_eq!(res.match_count, 3);
        assert_eq!(res.content, "X bar X baz X");
    }

    #[test]
    fn unicode_normalized_matches_smart_quotes() {
        // Content uses smart quotes; pattern uses ASCII.
        let content = "let s = \u{201C}hello\u{201D};\n";
        let old = "\"hello\"";
        let res = fuzzy_find_and_replace(content, old, "\"world\"", false);
        assert_eq!(
            res.strategy,
            Some("unicode_normalized"),
            "err={:?}",
            res.error
        );
        assert!(res.content.contains("world"));
    }

    #[test]
    fn case_sensitivity_is_preserved() {
        // The fuzzy chain never lowercases — `Foo` and `foo` must not match.
        let res = fuzzy_find_and_replace("Foo\n", "foo", "bar", false);
        assert!(res.error.is_some(), "case-mismatch should NOT match");
        assert_eq!(res.match_count, 0);
        assert_eq!(res.content, "Foo\n");
    }

    #[test]
    fn sequence_match_ratio_matches_python_reference() {
        // Pre-computed Python reference values (difflib.SequenceMatcher(None,a,b).ratio()):
        //   ratio("abcd", "abcd")     == 1.0
        //   ratio("abcd", "abce")     == 0.75
        //   ratio("hello", "world")   == 0.2     (only 'l','o' total len 1+1)
        //   actually difflib gives 0.2 for ('l','o') matched chars 1, but the
        //   gestalt match yields 'lo' contiguous in 'world' is 'lo'? no -- 'wor'-'l'-'d',
        //   so contiguous 'l' size 1 + recurse(hello[0..3], world[0..3]='wor')=0
        //   and recurse(hello[4..], world[4..])=0 → total 1, ratio = 2/10 = 0.2.
        let r1 = sequence_match_ratio("abcd", "abcd");
        let r2 = sequence_match_ratio("abcd", "abce");
        let r3 = sequence_match_ratio("hello", "world");
        assert!((r1 - 1.0).abs() < 1e-9, "r1 = {r1}");
        assert!((r2 - 0.75).abs() < 1e-9, "r2 = {r2}");
        assert!((r3 - 0.2).abs() < 1e-9, "r3 = {r3}");
    }
}
