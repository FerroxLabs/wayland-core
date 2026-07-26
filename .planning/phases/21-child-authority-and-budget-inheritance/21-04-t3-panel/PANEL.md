# 21-04 panel — how to grade Success Criterion 1

The single most consequential judgement in the phase verdict, and the one most
exposed to the forgery this plan was written to avoid: narrowing a criterion
until the evidence in hand satisfies it. Put to four independent members, three
external and one internal adversarial pass arguing AGAINST the emerging
consensus.

Question as asked: `question.txt`. Raw captures: `codex-sol.raw.txt`,
`gemini-pro.raw.txt`, `kimi-k3.raw.txt`.

PANEL-DECISION :: NOT-MET :: UNANIMOUS-ON-THE-EXTERNAL-LEGS, adversarial dissent recorded and lost

| Member | Position | Load-bearing reason |
|---|---|---|
| `codex-sol` (gpt-5.6-sol) | NOT-MET | "The criterion is universal and enforcement-based. Tool restrictions demonstrably widen in a product unit test, four production spawner sites remain fail-open, and PolicyGate is never enabled. NO-CHANNEL evidence for other dimensions cannot compensate for these open violations." |
| `gemini-3.1-pro` | NOT-MET | "The invariant explicitly requires that a child cannot widen restrictions across *any* of the enumerated dimensions, but the enforcement mechanisms remain structurally incomplete and fail-open." |
| `kimi-k3` | NOT-MET | "The tool dimension is affirmatively falsified: `build_tool_registry` registers Bash without consulting the parent registry, the fix was declined, and four production spawner sites remain fail-open." |
| internal adversarial | MET-WITH-STATED-EXCEPTIONS | argued below, and lost |

Extraction note, per this phase's measured traps: codex repeats its final block,
so its position was taken from the LAST match; kimi bullet-prefixes, so its
extraction was unanchored; gemini needed `--skip-trust` or it returns nothing.
All three were honoured.

## The adversarial case, stated at its strongest

Three arguments for `MET-WITH-STATED-EXCEPTIONS`, none of them frivolous:

1. **The three-way grade collapses to two.** `MET WITH STATED EXCEPTIONS` exists
   precisely so a criterion with named, bounded gaps can be graded without either
   overclaiming or flattening every partial state into failure. If any open
   exception forces `NOT-MET`, the middle verdict is unreachable and the scale is
   decorative.
2. **No amplification was ever OBSERVED.** The tool dimension is an absent guard,
   not a demonstrated widening: 21-02's corpus recorded REFUSED on every tool
   combination it could actually drive. Grading `NOT-MET` on a guard never shown
   to be exploitable overstates the risk in the opposite direction from the one
   this plan is guarding against.
3. **Consistency.** Criteria 2 and 3 are being graded
   `MET-WITH-STATED-EXCEPTIONS` on evidence that also carries named gaps and
   unprovable dimensions. Treating Criterion 1 differently needs a principled
   reason or it is just severity by mood.

## Why it lost

**On argument 1 — the quantifier is the principled difference.** Criterion 1 is
the only one of the three that carries a universal over an ENUMERATED list:
*cannot widen ANY of* eleven named restrictions. Criterion 2 names six events and
the evidence covers all six, with the exceptions being OBSERVABILITY of correct
behaviour rather than a missing guard. Criterion 3 asserts equivalence, which was
demonstrated over the decisive set. `MET WITH STATED EXCEPTIONS` stays reachable
for criteria whose exceptions are gaps in PROOF; it is not available to a
universal whose enumeration contains a member confirmed false.

**On argument 2 — "confirmed absent" is strictly stronger than "unmeasured".**
The tool gap is not a dimension nobody could measure. The product's own unit test
at `spawner.rs:4357` shows `build_tool_registry` registering Bash without
consulting a parent. The corpus could not exploit it only because any toolset
beyond the read-only floor forces `IsolatedMutation` and durable workspace
preparation refuses first in a hermetic non-repository workspace — recorded as
21-02's F21-02-04, which is an ENVIRONMENT artifact, not enforcement. A guard
confirmed not to exist is a widened restriction waiting for a caller, and
`PolicyGate` with zero callers is the same class.

**On argument 3 — consistency is satisfied by applying the same rule, not the
same label.** The rule applied to all three criteria is: grade against exactly
what the sentence says. Applied to a universal over an enumeration with a
falsified member, that rule yields `NOT-MET`. Applied to Criteria 2 and 3 it
yields `MET-WITH-STATED-EXCEPTIONS`. The labels differ because the sentences do.

## Bounding the result

The unanimity is worth discounting slightly and the discount is recorded rather
than hidden: all three external members were given the same framing, and that
framing already described the tool dimension as "CONFIRMED ABSENT" and
"affirmatively falsified" language they each echoed back. A summary written in
the opposite direction might have pulled differently. The framing is not
invented, though — it is 21-03's own recorded finding, in 21-03's own words
("Success Criterion 1 cannot be claimed for the tool dimension"), so the panel
was asked about the evidence as the phase itself established it.

The decision therefore rests on the quantifier argument, which is checkable from
`ROADMAP.md:79` alone and does not depend on the panel agreeing.
