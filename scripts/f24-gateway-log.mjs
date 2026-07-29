/**
 * f24-gateway-log.mjs — pure matchers over a `gateway run` log.
 *
 * Extracted as a module for F24-C3-H5 so the self-test can exercise the SHIPPED
 * matcher rather than a copy of it. A self-test that re-implements the thing it
 * is testing drifts away from the instrument silently, and this program has
 * already recorded an instrument defect that recurred because it was written up
 * instead of repaired.
 *
 * Both functions take the log TEXT, not a path, so they are trivially testable
 * against a fixture string with no filesystem involved.
 */

/**
 * The tool posture the product resolved for `channelName`, read from the
 * `channel turn dispatch` events `channel_dispatch.rs` emits on every admitted
 * turn.
 *
 * This observes the permission the turn genuinely ran under, not the one the
 * config asked for — the only surface that can tell the F24-C3-H5 repair apart
 * from the half-fix, because both of those deliver the message and only one
 * runs it under the configured posture.
 *
 * Returns `{posture, tier, sightings}`:
 *
 * - `posture: null` is NOT `"Conversational"`. No observation and an
 *   observation of the fallback are opposite findings, and collapsing them is
 *   exactly how a dead matcher reports the reassuring answer.
 * - `tier: 'same-line'` is authoritative — the shape tracing writes to a file.
 * - `tier: 'wrapped'` is a fallback for a sink that inserts a newline between
 *   the fields. Deliberately NOT the primary: a cross-line scan can attribute
 *   one channel's `channel=` to another's `posture=`, and a matcher that
 *   mis-attributes silently is worse than one that finds nothing.
 */
export function observedPostureIn(logText, channelName) {
  if (typeof logText !== 'string' || logText.length === 0) {
    return { posture: null, tier: 'no-log', sightings: 0 };
  }
  const esc = escapeRe(channelName);

  const sameLine = new RegExp(
    `channel turn dispatch[^\\n]*?channel\\s*=\\s*"?${esc}"?[^\\n]*?posture\\s*=\\s*([A-Za-z]+)`,
    'g',
  );
  let m;
  let last = null;
  let n = 0;
  while ((m = sameLine.exec(logText)) !== null) {
    last = m[1];
    n += 1;
  }
  if (last !== null) return { posture: last, tier: 'same-line', sightings: n };

  const wrapped = new RegExp(
    `channel turn dispatch[\\s\\S]{0,300}?channel\\s*=\\s*"?${esc}"?[\\s\\S]{0,300}?posture\\s*=\\s*([A-Za-z]+)`,
    'g',
  );
  while ((m = wrapped.exec(logText)) !== null) {
    last = m[1];
    n += 1;
  }
  return { posture: last, tier: last === null ? 'absent' : 'wrapped', sightings: n };
}

/**
 * How many `inbound denied` events the gateway logged for `channelName`.
 *
 * A "still denies" assertion written as "no new arrival" is self-passing on a
 * dead pipe, a wrong URL, a crashed gateway or a typo'd channel name — every
 * one of which produces the same zero. The denial has to be SIGHTED.
 */
export function denialsIn(logText, channelName) {
  if (typeof logText !== 'string' || logText.length === 0) return 0;
  const re = new RegExp(`inbound denied[^\\n]*?channel\\s*=\\s*"?${escapeRe(channelName)}"?`, 'g');
  return (logText.match(re) || []).length;
}

function escapeRe(s) {
  return String(s).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
