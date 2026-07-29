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
export function observedPostureIn(rawLogText, channelName) {
  if (typeof rawLogText !== 'string' || rawLogText.length === 0) {
    return { posture: null, tier: 'no-log', sightings: 0 };
  }
  const logText = stripAnsi(rawLogText);
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
export function denialsIn(rawLogText, channelName) {
  if (typeof rawLogText !== 'string' || rawLogText.length === 0) return 0;
  const re = new RegExp(`inbound denied[^\\n]*?channel\\s*=\\s*"?${escapeRe(channelName)}"?`, 'g');
  return (stripAnsi(rawLogText).match(re) || []).length;
}

/**
 * Remove ANSI SGR escapes.
 *
 * **This is an instrument repair, made after the instrument produced a false
 * absence — the exact failure class this program keeps rediscovering.**
 *
 * The first live control run of these matchers reported `posture=null,
 * sightings=0` for EVERY channel, and the leg dutifully graded FAIL. The line
 * was in the log the whole time. `tracing-subscriber` colourises field names
 * when it writes, so what is on disk is not
 *
 *     channel turn dispatch channel=f24finthree posture=Workspace
 *
 * but
 *
 *     channel turn dispatch \x1b[3mchannel\x1b[0m\x1b[2m=\x1b[0mf24finthree ...
 *
 * — the escape sequences sit BETWEEN the field name and the `=`, so
 * `channel\s*=` cannot match. Every posture read came back null, which is
 * indistinguishable from "the product never dispatched" and would have been
 * reported as a product finding by a less suspicious reading.
 *
 * It was caught only because the leg carried a positive control that ALSO read
 * null: a channel known to have been admitted (its reply is in the sink
 * journal) cannot honestly have no dispatch line, so the zero had to be the
 * instrument. That is the entire argument for pairing every zero with a
 * control.
 *
 * Repaired here rather than noted (§6b-ii), with three self-test assertions
 * over the VERBATIM bytes captured from that run.
 */
export function stripAnsi(s) {
  // eslint-disable-next-line no-control-regex
  return String(s).replace(/\x1b\[[0-9;]*[A-Za-z]/g, '');
}

function escapeRe(s) {
  return String(s).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
