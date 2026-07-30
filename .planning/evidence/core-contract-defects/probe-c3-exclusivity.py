#!/usr/bin/env python3
"""C3 adjudication: what makes the duplicate top-level branches exclusive, and
is the nested inferred level the ONLY place the exclusivity mechanism is absent?

Includes a REPAIRED events.rs variant-field parser with a 3-assertion self-test
(known-positive passes / known-negative fails / the OLD broken parser misses it).
"""
import json
import pathlib
import re
import sys

from jsonschema import Draft202012Validator

ROOT = pathlib.Path(__file__).resolve().parents[3]
C = ROOT / "crates/wcore-protocol/contracts/desktop/v1"
core = json.loads((C / "schema/core-event.schema.json").read_text())
prod = json.loads((C / "schema/producer-complete.schema.json").read_text())
SRC = (ROOT / "crates/wcore-protocol/src/events.rs").read_text()

out = []


def p(s=""):
    out.append(str(s))


# ---------------------------------------------------------------- instrument repair
def variant_fields_OLD(src, name):
    """The parser that returned [] for every variant. Kept ONLY as the third
    self-test assertion: a repair that the old parser would also have passed
    proves nothing."""
    i = src.index(f"\n    {name} {{")
    j = src.index("\n    },", i)
    body = src[i:j]
    fields, depth = [], 0
    for line in body.splitlines():
        s = line.strip()
        if s.startswith(("//", "#[")):
            continue
        if depth == 0 and ":" in s and not s.startswith("///"):
            fname = s.split(":")[0].strip()
            if fname.isidentifier():
                fields.append(fname)
        depth += s.count("{") - s.count("}")
    return fields


def variant_fields(src, name):
    """REPAIRED. The old version computed `depth` AFTER using it, and seeded it
    from the variant's own opening brace, so depth was >=1 on every field line
    and the `depth == 0` guard excluded all of them."""
    i = src.index(f"\n    {name} {{")
    body_start = src.index("{", i) + 1
    depth, j = 1, body_start
    while j < len(src) and depth:
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
        j += 1
    body = src[body_start : j - 1]
    fields, depth = [], 0
    for line in body.splitlines():
        s = line.strip()
        if depth == 0 and not s.startswith(("//", "#[", "///")):
            m = re.match(r"^([a-z_][a-z0-9_]*)\s*:", s)
            if m:
                fields.append(m.group(1))
        depth += s.count("{") - s.count("}")
    return fields


p("=== INSTRUMENT SELF-TEST: repaired events.rs field parser (3 assertions) ===")
a1 = variant_fields(SRC, "CorrelatedSubAgentEvent")
p(f"  (1) known-POSITIVE  CorrelatedSubAgentEvent -> {a1}")
assert "child_run_id" in a1 and "parent_call_id" in a1, "known-positive FAILED"
a2 = variant_fields(SRC, "ApprovalResume")
p(f"  (2) known-NEGATIVE  'child_run_id' in ApprovalResume {a2}: {'child_run_id' in a2}")
assert "child_run_id" not in a2, "known-negative FAILED"
a3 = variant_fields_OLD(SRC, "CorrelatedSubAgentEvent")
p(f"  (3) OLD parser on the same input -> {a3}")
assert a3 != a1 and not a3, "the old parser would NOT have missed it; repair unproven"
p("  all three assertions hold: the repair is load-bearing, not cosmetic.")

p()
p("=== What makes duplicate top-level branches mutually exclusive? ===")
for i in (26, 27):
    b = core["oneOf"][i]
    p(f"  core.oneOf[{i}] additionalProperties={b.get('additionalProperties')!r} "
      f"required={len(b.get('required', []))} props={len(b.get('properties', {}))}")

p()
p("  Cross-probe: does a CORRELATED sub_agent_event frame match the LEGACY branch?")
fx = json.loads((C / "events/sub_agent_event.json").read_text())
for i in (26, 27):
    errs = list(Draft202012Validator(core["oneOf"][i]).iter_errors(fx))
    p(f"    correlated fixture vs core.oneOf[{i}]: {len(errs)} err "
      f"{[list(e.schema_path) for e in errs][:2]}")

p("  Cross-probe: a synthetic LEGACY frame (correlated fields stripped):")
legacy = {k: v for k, v in fx.items() if k in {"type", "parent_call_id", "agent_name", "inner"}}
for i in (26, 27):
    errs = list(Draft202012Validator(core["oneOf"][i]).iter_errors(legacy))
    p(f"    legacy frame vs core.oneOf[{i}]: {len(errs)} err")
whole = list(Draft202012Validator(core).iter_errors(legacy))
p(f"    legacy frame vs WHOLE core-event.schema.json: {len(whole)} err "
  f"-> {'ACCEPTED' if not whole else 'REJECTED'}")

p()
p("=== approval_resume: are producer-complete anyOf[4] and anyOf[59] identical? ===")
b4, b59 = prod["anyOf"][4], prod["anyOf"][59]
p(f"  identical: {b4 == b59}")
p(f"  anyOf[4]  title={b4.get('title')!r} addlProps={b4.get('additionalProperties')!r}")
p(f"  anyOf[59] title={b59.get('title')!r} addlProps={b59.get('additionalProperties')!r}")
if b4 != b59:
    p(f"  keys differ: {sorted(set(b4) ^ set(b59))}")

p()
p("=== additionalProperties census: top-level branches vs nested inferred objects ===")


def census(node, at_top, acc):
    if isinstance(node, dict):
        if node.get("type") == "object" or "properties" in node:
            acc["true" if node.get("additionalProperties") is not False else "false"] += 1
        for v in node.values():
            census(v, False, acc)
    elif isinstance(node, list):
        for v in node:
            census(v, False, acc)


top = {"true": 0, "false": 0}
for b in core["oneOf"]:
    top["true" if b.get("additionalProperties") is not False else "false"] += 1
p(f"  core-event TOP-LEVEL branches: additionalProperties==false -> {top['false']}, "
  f"otherwise -> {top['true']}")

nested = {"true": 0, "false": 0}
for b in core["oneOf"]:
    for v in b.get("properties", {}).values():
        census(v, False, nested)
p(f"  core-event NESTED object schemas: additionalProperties==false -> {nested['false']}, "
  f"otherwise -> {nested['true']}")

p()
p("=== every nested `oneOf` in core-event.schema.json (the C5 class, swept) ===")
found = []


def walk(node, path):
    if isinstance(node, dict):
        if "oneOf" in node and path:
            found.append((path, node["oneOf"]))
        for k, v in node.items():
            walk(v, f"{path}/{k}")
    elif isinstance(node, list):
        for i, v in enumerate(node):
            walk(v, f"{path}/{i}")


walk(core, "")
p(f"  nested oneOf sites (excluding the root): {len(found)}")
unsat = 0
for path, branches in found:
    allperm = all(
        isinstance(b, dict)
        and b.get("additionalProperties") is not False
        and not b.get("required")
        and b.get("type") == "object"
        for b in branches
    )
    if allperm and len(branches) > 1:
        unsat += 1
        p(f"    UNSATISFIABLE-BY-CONSTRUCTION: {path} ({len(branches)} permissive object branches)")
p(f"  unsatisfiable-by-construction nested oneOf sites: {unsat}")

found2 = []


def walk2(node, path):
    if isinstance(node, dict):
        if "oneOf" in node and path:
            found2.append((path, node["oneOf"]))
        for k, v in node.items():
            walk2(v, f"{path}/{k}")
    elif isinstance(node, list):
        for i, v in enumerate(node):
            walk2(v, f"{path}/{i}")


walk2(prod, "")
unsat2 = 0
for path, branches in found2:
    allperm = all(
        isinstance(b, dict)
        and b.get("additionalProperties") is not False
        and not b.get("required")
        and b.get("type") == "object"
        for b in branches
    )
    if allperm and len(branches) > 1:
        unsat2 += 1
        p(f"    producer-complete UNSATISFIABLE: {path} ({len(branches)} branches)")
p(f"  producer-complete nested oneOf sites: {len(found2)}, unsatisfiable: {unsat2}")

sys.stdout.write("\n".join(out) + "\n")
