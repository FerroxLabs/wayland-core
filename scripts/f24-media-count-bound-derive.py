#!/usr/bin/env python3
"""Derive `f24-media-count-bound.mjs` from `f24-media-actions.mjs`.

The count-bound live leg needs the same gateway/fixture harness a prior lane
already built and proved, differing in exactly one dimension: how many
attachments the inbound MESSAGE_CREATE carries. Copying 489 lines by hand
would fork the harness and let the two drift; deriving it by patch keeps the
proven parts byte-identical and makes the delta reviewable.

Every substitution asserts its anchor. A patch that silently matched nothing
would emit a driver identical to the original, and the run would then measure
the wrong thing while looking healthy - the same self-passing shape this lane
is fixing in the product.
"""

import sys

SRC = "scripts/f24-media-actions.mjs"
DST = "scripts/f24-media-count-bound.mjs"

src = open(SRC).read()
o = src


def sub1(old, new, tag):
    global o
    if old not in o:
        sys.exit("ANCHOR MISSING (%s) - refusing to emit a driver from a patch "
                 "that did not apply" % tag)
    o = o.replace(old, new, 1)


sub1(
    "async function runLeg({ label, ack, withAttachment, binary, rootDir, budgetMs }) {",
    "async function runLeg({ label, ack, attachmentCount, binary, rootDir, budgetMs }) {",
    "runLeg signature",
)

sub1(
    """    const attachments = withAttachment
      ? [
          {
            url: 'https://cdn.discordapp.com/attachments/1/2/f24ma-probe.png',
            content_type: 'image/png',
          },
        ]
      : [];""",
    """    const attachments = Array.from({ length: attachmentCount }, (_, i) => ({
      url: `https://cdn.discordapp.com/attachments/1/2/f24mcb-probe-${i + 1}.png`,
      content_type: 'image/png',
    }));""",
    "attachment array",
)

sub1(
    "    with_attachment: Boolean(withAttachment),",
    """    attachment_count: attachmentCount,
    // The past-bound notice emitted by ChannelMediaEnricher::enrich. Asserted
    // on a SHORT phrase: the Rust literal uses backslash line-continuations, so
    // transcribing it whole out of the source would embed a newline and an
    // indent the runtime string does not contain - the exact matcher hazard a
    // prior lane measured on this same notice family.
    prompt_carries_count_bound_notice: turnPrompts.some((t) => t.includes(COUNT_BOUND_PHRASE)),
    count_bound_notice_hits: turnPrompts.reduce(
      (n, t) => n + (t.split(COUNT_BOUND_PHRASE).length - 1),
      0,
    ),""",
    "output fields",
)

sub1(
    "async function runLeg(",
    """const COUNT_BOUND_PHRASE = 'declared bound of 10 attachments per message';

async function runLeg(""",
    "phrase const",
)

start = o.find("  // A: the positive leg")
end = o.find("  const summary = {")
if start < 0 or end < 0 or end <= start:
    sys.exit("ANCHOR MISSING (legs/gates block)")

o = o[:start] + """  // A: POSITIVE - 12 attachments against discord's DECLARED bound of 10.
  const A = await runLeg({ label: 'A-12-attachments-over-bound', ack: 'off', attachmentCount: 12, binary, rootDir: outDir, budgetMs });
  // B: NEGATIVE CONTROL - ONLY the attachment count differs from A.
  const B = await runLeg({ label: 'B-3-attachments-under-bound', ack: 'off', attachmentCount: 3, binary, rootDir: outDir, budgetMs });

  const gates = [
    {
      id: 'C1',
      clause: 'declared max_attachments is enforced',
      kind: 'POSITIVE',
      desc: '12 inbound attachments against a declared bound of 10: the turn prompt carries the past-bound notice for EXACTLY the 2 over the bound',
      pass: A.identified && A.turn_ran && A.prompt_carries_count_bound_notice && A.count_bound_notice_hits === 2,
      detail: `identified=${A.identified} turn_ran=${A.turn_ran} notice=${A.prompt_carries_count_bound_notice} hits=${A.count_bound_notice_hits} (want exactly 2)`,
    },
    {
      id: 'C2',
      clause: 'declared max_attachments is enforced',
      kind: 'NEGATIVE CONTROL',
      desc: '3 attachments, one variable changed: notice ABSENT - and the prompt capture proven alive on the same run',
      pass: B.identified && B.turn_ran && B.prompt_carries_probe_text && B.prompt_carries_count_bound_notice === false,
      detail: `identified=${B.identified} turn_ran=${B.turn_ran} capture_alive=${B.prompt_carries_probe_text} notice=${B.prompt_carries_count_bound_notice}`,
    },
    {
      id: 'C3',
      clause: 'the in-bound attachments were still processed',
      kind: 'LIVENESS CONTROL',
      desc: 'the first 10 are NOT skipped - they carry the ordinary no-vision notice, proving the enricher ran rather than bailing out early',
      pass: A.turn_ran && A.prompt_carries_notice,
      detail: `turn_ran=${A.turn_ran} prompts=${A.turn_prompt_count} vision_notice=${A.prompt_carries_notice}`,
    },
  ];

""" + o[end:]

if "legs: { A, B, C }," not in o:
    sys.exit("ANCHOR MISSING (legs summary)")
o = o.replace("legs: { A, B, C },", "legs: { A, B },", 1)

if o == src:
    sys.exit("PATCH WAS A NO-OP - refusing to emit")

open(DST, "w").write(o)
print("DERIVED %s (%d bytes from %d)" % (DST, len(o), len(src)))
