# Instrument-repair note (lane brief 6b-ii): the first version of this check
# lived inline in livetest.sh as
#     echo "--- 3.1-only marker: OpenAPI 3.1 replaces `nullable: true` ... ---"
# Backticks inside a double-quoted shell string are COMMAND SUBSTITUTION, so the
# shell tried to EXECUTE the phrase the line was meant to print
# ("nullable:: command not found") and the label was silently mangled. The check
# printed "0", which happens to be the right answer for the wrong reason: it was
# counting in a line whose surrounding text had already been destroyed, and a
# reader could not tell whether 0 meant "3.1 shape confirmed" or "grep never ran".
# Repaired here as a real DIFFERENTIAL with a self-test below.
import json, sys


def count_31_form(o):
    """OpenAPI 3.1 spells an optional field as a type ARRAY containing "null"."""
    c = 0
    if isinstance(o, dict):
        t = o.get("type")
        if isinstance(t, list) and "null" in t:
            c += 1
        for v in o.values():
            c += count_31_form(v)
    elif isinstance(o, list):
        for v in o:
            c += count_31_form(v)
    return c


def count_30_form(raw):
    """OpenAPI 3.0 spells it "nullable": true."""
    return raw.count('"nullable"')


def self_test():
    """Three assertions, not two — the third is the only one that proves the
    repair does anything, because the OLD broken instrument would pass the
    first two as well."""
    doc_31 = {"components": {"schemas": {"S": {"properties": {"x": {"type": ["string", "null"]}}}}}}
    doc_30 = {"components": {"schemas": {"S": {"properties": {"x": {"type": "string", "nullable": True}}}}}}

    # 1. known-positive: a real 3.1 document is detected.
    assert count_31_form(doc_31) == 1, "known-positive failed"
    # 2. known-negative: a 3.0 document is NOT counted as 3.1.
    assert count_31_form(doc_30) == 0, "known-negative failed"
    # 3. the OLD instrument could not tell these apart. It only ever counted the
    #    substring '"nullable"', which is 0 for BOTH a correct 3.1 document and a
    #    document that failed to render at all (empty file, 404 body, truncated
    #    stream). Prove that: the old matcher scores an EMPTY document identically
    #    to a good 3.1 document, so it could never have detected the failure mode
    #    it was supposed to guard.
    old_score_good_31 = count_30_form(json.dumps(doc_31))
    old_score_empty = count_30_form("")
    assert old_score_good_31 == old_score_empty == 0, "premise of assertion 3 wrong"
    # ...whereas the repaired instrument separates them:
    assert count_31_form(doc_31) == 1 and count_31_form(json.loads("{}")) == 0, \
        "repair does not distinguish good-3.1 from empty"
    print("  SELF_TEST=PASS (known-positive, known-negative, and old-matcher-blind)")


if __name__ == "__main__":
    self_test()
    raw = open(sys.argv[1]).read()
    d = json.loads(raw)
    n30, n31 = count_30_form(raw), count_31_form(d)
    print(f"  count_3_0_form_nullable_true = {n30}   (must be 0 under 3.1)")
    print(f"  count_3_1_form_type_null     = {n31}   (must be > 0, else the doc")
    print("                                          only changed its version")
    print("                                          string, not its shape)")
    print(f"  SHAPE_DIFFERENTIAL={'PASS' if (n30 == 0 and n31 > 0) else 'FAIL'}")
