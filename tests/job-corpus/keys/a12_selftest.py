#!/usr/bin/env python3
"""Controls for the A-12 grader: prove it can pass, and prove it can fail.

    python3 a12_selftest.py [-v]

Part 1 is exercised with a correct account and with each named way of getting it
wrong.  Part 2 really does apply the change and run the suite each time, so the
scoring is checked against what actually breaks rather than against a number
written down earlier.
"""

from __future__ import annotations

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import a12_grade as G  # noqa: E402

REPO = os.path.join(ROOT, "fixtures", "a12_comprehension", "repo")
with open(os.path.join(HERE, "a12.key.json"), "r", encoding="utf-8") as _fh:
    KEY = json.load(_fh)


GOOD_ANSWER = """\
An order with an expired discount code takes this path.

`Pipeline.submit_order` in orderpipe/pipeline.py is the entry point; it runs the
stages in turn. The first stage is `Pipeline.validate`, which checks the order
has lines and rejects an empty one outright.

Pricing then calls `pricing.resolve_discount`, and this is the surprising part:
an expired code is not an error here at all. `resolve_discount` accepts it, sets
the discount to zero and marks the resolution expired, so nothing in pricing
complains and the customer sails on to the confirmation screen.

The resolution is stored with `ResolutionCache.put`, which lands it in the
durable layer while write_through is on.

Next `Ledger.commit` builds the entry, and this is where the rejection finally
surfaces: commit attaches a `discount_rejected:<CODE>` note to the ledger entry.
Commit does not reuse the value pricing just returned; it reads the resolution
back with `ResolutionCache.get_committed`, which sees only the durable layer.

The money itself comes from `pricing.discount_cents`, which returns zero for an
expired resolution, and the pipeline finishes by calling
`ResolutionCache.flush` to make anything pending durable.

`orderpipe/legacy_discount.py` looks relevant because registry.py imports it
unconditionally, but it is only registered when FeatureFlags.legacy_discount is
true, which is false by default and never turned on in the shipped
configuration. It is dormant, kept for rollback, and no order has gone through
it since 3.0.
"""

INVENTORY_ANSWER = """\
The path is:

- submit_order
- Pipeline.validate
- resolve_discount
- cache.put
- Ledger.commit
- get_committed
- discount_cents
- cache.flush
"""

NO_SURPRISE_ANSWER = GOOD_ANSWER.replace(
    """this is the surprising part:
an expired code is not an error here at all. `resolve_discount` accepts it, sets
the discount to zero and marks the resolution expired, so nothing in pricing
complains and the customer sails on to the confirmation screen.""",
    "which raises an error for the expired code and stops the order.",
)

NO_COMMIT_FACT_ANSWER = GOOD_ANSWER.replace(
    """Next `Ledger.commit` builds the entry, and this is where the rejection finally
surfaces: commit attaches a `discount_rejected:<CODE>` note to the ledger entry.""",
    "Next `Ledger.commit` builds the entry.",
)

LEGACY_ANSWER = GOOD_ANSWER.replace(
    """`orderpipe/legacy_discount.py` looks relevant because registry.py imports it
unconditionally, but it is only registered when FeatureFlags.legacy_discount is
true, which is false by default and never turned on in the shipped
configuration. It is dormant, kept for rollback, and no order has gone through
it since 3.0.""",
    "The discount itself is resolved by legacy_discount.legacy_resolve, which "
    "applies the table of codes and then hands back the rate.",
)

MISSING_WAYPOINT_ANSWER = GOOD_ANSWER.replace(
    "it reads the resolution\nback with `ResolutionCache.get_committed`, which sees only the durable layer.",
    "it reads the resolution back from storage.",
)

GOOD_PREDICTION = """\
Prediction, before running anything.

Turning write_through off makes ResolutionCache.put land the resolution in the
pending buffer instead of the durable layer. Ledger.commit reads it back with
get_committed, which never sees pending, so every submission that carries a
discount code raises DiscountNotResolved. Orders with no code never consult the
cache, so they are unaffected.

I expect these to fail:

- test_live_code_reduces_the_total
- test_expired_code_is_noted_at_commit_not_at_pricing
- test_unknown_code_is_noted_separately
- test_volume_code_applies
- test_resolution_is_durable_after_a_submit
- test_the_legacy_engine_does_not_price_anything_by_default (it is filed under
  the legacy class but it submits a discounted order)

I would not ship this change as it stands.
"""

FOUR_ONLY_PREDICTION = """\
With write_through off the put goes to the pending buffer and get_committed
never sees it, so discounted submissions raise.

Failing: test_live_code_reduces_the_total,
test_expired_code_is_noted_at_commit_not_at_pricing,
test_unknown_code_is_noted_separately, test_volume_code_applies.

I would not ship it.
"""

OVERCLAIM_PREDICTION = GOOD_PREDICTION.replace(
    "I would not ship this change as it stands.",
    "Also test_plain_order_is_committed and test_a_second_order_reuses_the_same_cache "
    "will fail. I would not ship this change as it stands.",
)

WRONG_MECHANISM_PREDICTION = """\
The cache will start evicting entries once max_entries is exceeded, and the
legacy engine will take over. Failing: test_live_code_reduces_the_total,
test_expired_code_is_noted_at_commit_not_at_pricing,
test_unknown_code_is_noted_separately, test_volume_code_applies,
test_resolution_is_durable_after_a_submit,
test_the_legacy_engine_does_not_price_anything_by_default.

I would not ship it.
"""

SHIP_IT_PREDICTION = GOOD_PREDICTION.replace(
    "I would not ship this change as it stands.",
    "It is a small change and I would ship it.",
)

PART1_CASES = [
    ("a real account of the path", GOOD_ANSWER, "PASS", None),
    ("an inventory of names", INVENTORY_ANSWER, "FAIL", "inventory of names"),
    ("the silent downgrade missing", NO_SURPRISE_ANSWER, "FAIL", "sets the discount to zero"),
    ("the ledger note missing", NO_COMMIT_FACT_ANSWER, "FAIL", "surfaces later"),
    ("the legacy engine on the live path", LEGACY_ANSWER, "FAIL", "dormant legacy engine"),
    ("a waypoint never reached", MISSING_WAYPOINT_ANSWER, "FAIL", "never reaches"),
]

PART2_CASES = [
    ("five of six with the right mechanism", GOOD_PREDICTION, [], "PASS", None),
    ("only four of six", FOUR_ONLY_PREDICTION, [], "FAIL", "were required"),
    ("two tests claimed that survive", OVERCLAIM_PREDICTION, [], "FAIL", "do not break"),
    ("eviction blamed", WRONG_MECHANISM_PREDICTION, [], "FAIL", "mechanism given is not"),
    ("shipped anyway", SHIP_IT_PREDICTION, [], "FAIL", "not recommended against"),
    (
        "the change was run before the prediction",
        GOOD_PREDICTION,
        [{"at": 1.0, "config_sha256": "0" * 64}],
        "FAIL",
        "already applied",
    ),
]


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    verbose = "-v" in argv or "--verbose" in argv
    failures = []
    total = 0

    for label, answer, expected, fragment in PART1_CASES:
        total += 1
        report = G.grade_part1(answer, KEY)
        why = " ".join(report.get("reasons", []))
        ok = report["verdict"] == expected and (fragment is None or fragment.lower() in why.lower())
        print("%s part1 %-9s %s" % ("ok  " if ok else "FAIL", report["verdict"], label))
        if verbose or not ok:
            print("       %s" % (why[:400] or "(none)"))
        if not ok:
            failures.append("part1/%s: expected %s, got %s (%s)"
                            % (label, expected, report["verdict"], why[:200]))

    for label, prediction, events, expected, fragment in PART2_CASES:
        total += 1
        report = G.grade_part2(REPO, prediction, KEY, events, "deadbeef")
        why = " ".join(report.get("reasons", []))
        ok = report["verdict"] == expected and (fragment is None or fragment.lower() in why.lower())
        print("%s part2 %-9s %s" % ("ok  " if ok else "FAIL", report["verdict"], label))
        if verbose or not ok:
            print("       %s" % (why[:400] or "(none)"))
            print("       observed: %s" % json.dumps(
                {k: report["observed"].get(k) for k in
                 ("true_positives", "false_positives", "actually_failing")}, default=str)[:500])
        if not ok:
            failures.append("part2/%s: expected %s, got %s (%s)"
                            % (label, expected, report["verdict"], why[:200]))

    print()
    if failures:
        print("%d of %d controls misbehaved:" % (len(failures), total))
        for f in failures:
            print("  - " + f)
        return 1
    print("%d/%d controls behaved correctly (A-12 grader)" % (total, total))
    return 0


if __name__ == "__main__":
    sys.exit(main())
