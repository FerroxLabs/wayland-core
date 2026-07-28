#!/usr/bin/env node
// Correlation-token matching for the Phase 24 inbound matrix — and the
// `instrument_fault` state that keeps a mangled token from being reported as a
// lost message.
//
// WHY THIS FILE EXISTS.
//
// Every arrival count in `f24-inbound.mjs` is derived by searching an
// out-of-process journal for the correlation token the leg planted. Until this
// file, that search was `text.includes(token)` — an EXACT substring test.
//
// That test is wrong for at least one adapter that ships today, and it is wrong
// silently. `crates/wcore-channel-telegram/src/config.rs` defaults
// `parse_mode` to `MarkdownV2` (`default_parse_mode`, line 60) and
// `lib.rs:274-277` runs `escape_markdown_v2()` over the FULL outbound body under
// that mode. `-` is in the reserved set (config.rs:105-107). The matrix's own
// tokens look like `f24c3-telegram-admit-9f3a1c02` — four hyphens — so a reply
// that arrived perfectly intact reaches the journal as
// `f24c3\-telegram\-admit\-9f3a1c02` and an exact matcher scores it ZERO.
//
// A lane on this program already hit exactly that and reported `replied=0` for
// eight replies that had all arrived. LANE-BRIEF §6b-ii is explicit that
// writing such a defect up is not a fix — the instrument gets repaired in the
// same lane, because the one measured recurrence of this class happened
// precisely because the earlier sighting was documented instead of repaired.
//
// THE THREE TIERS, AND WHY THE THIRD ONE IS THE POINT.
//
//   1. exact       the token appears verbatim.
//   2. normalized  the token appears after undoing a transport encoding this
//                  module KNOWS about (MarkdownV2 backslash-escaping, HTML
//                  entity escaping). Still a real arrival.
//   3. fuzzy       the token's alphanumeric skeleton appears, but neither of
//                  the above matched. Something transformed the text in a way
//                  this module does NOT model.
//
// Tier 3 is not a looser way to pass. It is a DETECTOR. A tier-3 hit means the
// instrument is not modelling the transport, so the run's numbers cannot be
// trusted in either direction — and the correct grade is INCOMPLETE, not LOSS.
// Grading it LOSS would blame the product for the harness's blind spot; grading
// it PASS would let a genuinely broken reply through. Neither is honest, so the
// state is its own third outcome.
//
// The failure mode this guards against is asymmetric and worth stating: a
// missed match turns a WORKING adapter into a reported defect, which costs a
// repair cycle chasing nothing. That is how the eleven recorded instances of
// "the instrument carries the class it hunts" have all played out.
//
// NOTE ON THE REJECTED ALTERNATIVE. The cheap fix is to mint tokens containing
// no MarkdownV2-reserved characters, so escaping cannot touch them. That is
// rejected deliberately: it makes THIS defect invisible rather than detected,
// and leaves the next unanticipated encoding — a future adapter that
// percent-encodes, or normalises unicode, or wraps at a column — silently
// scoring zero exactly as MarkdownV2 did.

/// The characters Telegram reserves under MarkdownV2. Mirrors the `RESERVED`
/// constant in `crates/wcore-channel-telegram/src/config.rs:105`. Kept as a
/// literal copy rather than derived: this module must be able to detect an
/// escaping the product performs even if the product's set later drifts.
export const MARKDOWN_V2_RESERVED = [
  '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
];

const RESERVED_SET = new Set(MARKDOWN_V2_RESERVED);

/// Undo `escape_markdown_v2`: drop any backslash that immediately precedes a
/// reserved character. A backslash before a NON-reserved character is left
/// alone, because the product does not introduce one there — matching the
/// Rust function's documented behaviour (`a\b.c` -> `a\b\.c`).
export function unescapeMarkdownV2(s) {
  let out = '';
  for (let i = 0; i < s.length; i += 1) {
    if (s[i] === '\\' && i + 1 < s.length && RESERVED_SET.has(s[i + 1])) {
      out += s[i + 1];
      i += 1;
      continue;
    }
    out += s[i];
  }
  return out;
}

/// Undo `escape_html`. `&amp;` LAST, mirroring the inverse order of the Rust
/// function which replaces `&` first.
export function unescapeHtml(s) {
  return s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
}

/// Reduce to a lowercase alphanumeric skeleton. Everything a transport might
/// insert between token characters — backslashes, entity text, zero-width
/// joiners, line wraps — falls out, while the token's own letters and digits
/// survive in order.
export function skeleton(s) {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, '');
}

/// Length of the SHORTEST window of `hay` that contains `needle` as a
/// subsequence, or `Infinity` if there is none.
///
/// A plain substring test on skeletons is not enough, and finding that out was
/// the point of writing the self-test first. Skeletonising deletes only
/// NON-alphanumeric noise, so it catches a spliced zero-width character or a
/// console line-wrap — but it does NOT catch a transformation that inserts
/// alphanumerics. Percent-encoding is exactly that: `-` becomes `%2D`, whose
/// `2` and `D` survive skeletonisation and break the substring. The first draft
/// of this module scored that case `absent`, which is the same silent zero the
/// whole file exists to prevent, one encoding further along.
///
/// A subsequence test with a window bound covers both families: the token's own
/// characters must appear in order, and they must appear close enough together
/// that an accidental scatter through unrelated prose cannot qualify.
export function minSubsequenceWindow(hay, needle) {
  if (needle.length === 0) return 0;
  // Bound the scan. A pathological journal line must not turn the detector into
  // the reason a run times out.
  if (hay.length > 50_000) return Infinity;
  let best = Infinity;
  for (let start = 0; start < hay.length; start += 1) {
    if (hay[start] !== needle[0]) continue;
    if (hay.length - start >= best) break;
    let j = 0;
    for (let i = start; i < hay.length; i += 1) {
      if (hay[i] !== needle[j]) continue;
      j += 1;
      if (j === needle.length) {
        const width = i - start + 1;
        if (width < best) best = width;
        break;
      }
    }
  }
  return best;
}

/// How much longer than the token itself a mangled rendering may be before the
/// detector stops believing it is the same token. Percent-encoding every
/// separator inflates by roughly 1.25x; `&#xNN;`-style numeric entities by
/// about 3x. Beyond 4x, an "in-order subsequence" is more plausibly a
/// coincidence than an encoding.
export const FUZZY_WINDOW_FACTOR = 4;

/// How a correlation token appears in a piece of text.
///
/// Returns one of `'exact'`, `'normalized'`, `'fuzzy'`, `'absent'`.
export function classify(text, token) {
  const t = String(text ?? '');
  if (t.includes(token)) return 'exact';
  const normalized = unescapeHtml(unescapeMarkdownV2(t));
  if (normalized.includes(token)) return 'normalized';
  // A skeleton test on a token whose skeleton is trivially short would collide
  // with ordinary prose. The matrix's tokens carry an 8-hex-character run, so
  // require a skeleton long enough that an accidental hit is not credible.
  const sk = skeleton(token);
  if (sk.length < 12) return 'absent';
  const hay = skeleton(t);
  if (hay.includes(sk)) return 'fuzzy';
  if (minSubsequenceWindow(hay, sk) <= sk.length * FUZZY_WINDOW_FACTOR) return 'fuzzy';
  return 'absent';
}

/// True when the token is present in a form the instrument understands — i.e.
/// this is a genuine arrival and may be counted.
export function matches(text, token) {
  const c = classify(text, token);
  return c === 'exact' || c === 'normalized';
}

/// True when the token is present in a form the instrument does NOT understand.
///
/// This is the INCOMPLETE signal. It is deliberately NOT counted as an arrival
/// (the harness cannot claim to have read a message it cannot decode) and
/// deliberately NOT counted as a loss (the message plainly reached the journal).
export function instrumentFault(text, token) {
  return classify(text, token) === 'fuzzy';
}

/// The matcher this module replaces, kept executable so the self-test can
/// assert the repair actually changes an outcome. Never call this from the
/// driver.
export function legacyMatches(text, token) {
  return String(text ?? '').includes(token);
}

/// Partition a list of journal records against one token.
///
/// `records` are `{text, conversation_id}` shaped. Returns the arrivals that
/// may be counted, plus every record that tripped the fault detector, so a
/// caller can grade the leg INCOMPLETE and print what it could not decode.
export function partition(records, token) {
  const arrivals = [];
  const faults = [];
  for (const r of records) {
    const c = classify(r.text, token);
    if (c === 'exact' || c === 'normalized') arrivals.push({ ...r, match: c });
    else if (c === 'fuzzy') faults.push({ ...r, match: c });
  }
  return { arrivals, faults };
}
