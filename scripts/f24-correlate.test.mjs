// Self-test for the repaired correlation matcher (LANE-BRIEF §6b-ii).
//
// The brief's requirement is specific and it is not the obvious two-assertion
// pair. A self-test with only "known-positive passes" and "known-negative
// fails" PASSES ON THE BROKEN INSTRUMENT TOO, because the broken instrument
// also accepts a verbatim token and also rejects an absent one. Such a test
// proves nothing about the repair.
//
// So every behaviour below is asserted THREE ways:
//
//   1. known-positive  the repaired matcher accepts the real transport encoding
//   2. known-negative  the repaired matcher still refuses an absent token, and
//                      does NOT hide the absence behind the fault state
//   3. differential    the OLD matcher would have MISSED the known-positive
//
// Assertion 3 is the only one that could fail if the repair were reverted, and
// it is therefore the only one that proves the repair does anything.

import test from 'node:test';
import assert from 'node:assert/strict';

import {
  classify,
  matches,
  instrumentFault,
  legacyMatches,
  partition,
  unescapeMarkdownV2,
  unescapeHtml,
  skeleton,
  MARKDOWN_V2_RESERVED,
} from './f24-correlate.mjs';

// A token of exactly the shape `f24-inbound.mjs` mints, hyphens and all.
const TOKEN = 'f24c3-telegram-admit-9f3a1c02';

/// Reimplementation of `escape_markdown_v2` from
/// `crates/wcore-channel-telegram/src/config.rs:103`. Written independently
/// here so the test drives the PRODUCT's transformation rather than reusing the
/// module under test's own inverse — an inverse tested against itself is a
/// tautology, which is the same defect class this file exists to close.
function escapeMarkdownV2(s) {
  let out = '';
  for (const ch of s) {
    if (MARKDOWN_V2_RESERVED.includes(ch)) out += '\\';
    out += ch;
  }
  return out;
}

test('the escaping fixture reproduces the product transformation', () => {
  // Anchored against the exact expectation asserted in the Rust unit test
  // `escape_markdown_v2_realistic_reply`, so a drift in this file's model of
  // the product is caught here rather than mis-attributed to the matcher.
  assert.equal(escapeMarkdownV2("Hello! I'm here. (ready)"), "Hello\\! I'm here\\. \\(ready\\)");
  assert.equal(escapeMarkdownV2('abc 123 XYZ'), 'abc 123 XYZ');
  assert.equal(escapeMarkdownV2('a\\b.c'), 'a\\b\\.c');
});

// ── 1. known-positive ────────────────────────────────────────────────────────

test('1 known-positive: a MarkdownV2-escaped reply is counted as an arrival', () => {
  const wire = escapeMarkdownV2(`ack ${TOKEN}`);
  // Precondition: the transport really did mangle it. Without this the test
  // could be quietly asserting on an unmangled string.
  assert.notEqual(wire, `ack ${TOKEN}`, 'fixture must actually escape the token');
  assert.equal(wire, 'ack f24c3\\-telegram\\-admit\\-9f3a1c02');

  assert.equal(classify(wire, TOKEN), 'normalized');
  assert.equal(matches(wire, TOKEN), true);
  assert.equal(instrumentFault(wire, TOKEN), false, 'a decodable arrival is not a fault');
});

test('1b known-positive: a verbatim reply still matches, exactly', () => {
  const wire = `ack ${TOKEN}`;
  assert.equal(classify(wire, TOKEN), 'exact');
  assert.equal(matches(wire, TOKEN), true);
});

test('1c known-positive: an HTML-escaped reply is counted', () => {
  const wire = `ack &lt;${TOKEN}&gt;`;
  assert.equal(matches(wire, TOKEN), true);
  assert.equal(unescapeHtml(wire), `ack <${TOKEN}>`);
});

// ── 2. known-negative ────────────────────────────────────────────────────────

test('2 known-negative: an absent token is absent, and is NOT excused as a fault', () => {
  const other = 'ack f24c3-telegram-access-9f3a1c02';
  assert.equal(classify(other, TOKEN), 'absent');
  assert.equal(matches(other, TOKEN), false);
  // The load-bearing half. If the fault detector fired here, a genuinely lost
  // message would be graded INCOMPLETE and the defect would be excused.
  assert.equal(instrumentFault(other, TOKEN), false, 'a real zero must stay a real zero');
});

test('2b known-negative: an empty / missing journal text is absent, not a fault', () => {
  for (const wire of ['', null, undefined, 'unrelated chatter']) {
    assert.equal(matches(wire, TOKEN), false);
    assert.equal(instrumentFault(wire, TOKEN), false);
  }
});

test('2c known-negative: a near-miss token does not match', () => {
  // One character of the random tag differs. The skeleton test must not paper
  // over this or two legs of the same run would contaminate each other.
  const near = escapeMarkdownV2('ack f24c3-telegram-admit-9f3a1c03');
  assert.equal(matches(near, TOKEN), false);
  assert.equal(instrumentFault(near, TOKEN), false);
});

// ── 3. differential — the old matcher would have missed it ───────────────────

test('3 differential: the OLD matcher misses the escaped reply the new one finds', () => {
  const wire = escapeMarkdownV2(`ack ${TOKEN}`);

  assert.equal(
    legacyMatches(wire, TOKEN),
    false,
    'the pre-repair matcher must MISS this — if it does not, the repair is inert',
  );
  assert.equal(
    matches(wire, TOKEN),
    true,
    'the repaired matcher must find it',
  );

  // Stated as the number the driver would have reported, because that is the
  // form the defect took in the field: `replied=0` against eight real arrivals.
  const journal = Array.from({ length: 8 }, (_, i) => ({
    text: escapeMarkdownV2(`ack ${TOKEN} #${i}`),
    conversation_id: '24030001',
  }));
  const legacyCount = journal.filter((r) => legacyMatches(r.text, TOKEN)).length;
  const repairedCount = partition(journal, TOKEN).arrivals.length;
  assert.equal(legacyCount, 0, 'the reported-zero the field defect produced');
  assert.equal(repairedCount, 8, 'every one of those eight replies had in fact arrived');
});

// ── the instrument_fault state itself ────────────────────────────────────────

test('instrument_fault fires on an encoding the matcher does NOT model', () => {
  // Percent-encoded hyphens. Nothing in this module decodes `%2D`, which is
  // precisely the point: it stands in for the NEXT unanticipated transport
  // transformation, the one after MarkdownV2.
  const wire = `ack ${TOKEN.replace(/-/g, '%2D')}`;
  assert.equal(matches(wire, TOKEN), false, 'must not be counted as a decoded arrival');
  assert.equal(instrumentFault(wire, TOKEN), true, 'must be flagged, not silently zeroed');
  assert.equal(classify(wire, TOKEN), 'fuzzy');
});

test('instrument_fault fires on zero-width characters spliced into the token', () => {
  const wire = `ack ${TOKEN.split('').join('​')}`;
  assert.equal(matches(wire, TOKEN), false);
  assert.equal(instrumentFault(wire, TOKEN), true);
});

test('instrument_fault fires on a console line-wrap inside the token', () => {
  // The exact shape recorded in LANE-BRIEF §6b-ii: a newline landing inside the
  // phrase the matcher searched for.
  const wire = `ack f24c3-telegram-adm\nit-9f3a1c02`;
  assert.equal(legacyMatches(wire, TOKEN), false);
  assert.equal(matches(wire, TOKEN), false);
  assert.equal(instrumentFault(wire, TOKEN), true, 'the wrap must be visible, not counted as loss');
});

test('partition separates countable arrivals from undecodable ones', () => {
  const journal = [
    { text: escapeMarkdownV2(`ack ${TOKEN}`), conversation_id: '24030001' },
    { text: `ack ${TOKEN.replace(/-/g, '%2D')}`, conversation_id: '24030001' },
    { text: 'ack something-else-entirely', conversation_id: '24030001' },
  ];
  const { arrivals, faults } = partition(journal, TOKEN);
  assert.equal(arrivals.length, 1);
  assert.equal(arrivals[0].match, 'normalized');
  assert.equal(faults.length, 1);
  assert.equal(faults[0].match, 'fuzzy');
});

// ── helpers ──────────────────────────────────────────────────────────────────

test('unescapeMarkdownV2 leaves a backslash before a non-reserved char alone', () => {
  // Mirrors the Rust test `escape_markdown_v2_backslash_prefix_is_correct`.
  assert.equal(unescapeMarkdownV2('a\\b\\.c'), 'a\\b.c');
});

test('unescapeMarkdownV2 round-trips every reserved character', () => {
  for (const ch of MARKDOWN_V2_RESERVED) {
    assert.equal(unescapeMarkdownV2(escapeMarkdownV2(ch)), ch, `reserved char ${ch}`);
  }
});

test('skeleton is too short to trip the fuzzy tier on trivial tokens', () => {
  // Guard on the length floor in `classify`. A short token must NOT be
  // fuzzy-matchable or ordinary reply prose would raise spurious faults.
  assert.ok(skeleton('ab-cd').length < 12);
  assert.equal(classify('the quick brown abcd fox', 'ab-cd'), 'absent');
});
