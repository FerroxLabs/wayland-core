#!/usr/bin/env python3
"""C3 refinement: are the duplicate discriminator branches distinguishable, and
are the legacy forms of workflow_started/workflow_finished representable at all?"""
import json
import pathlib
import sys

from jsonschema import Draft202012Validator

ROOT = pathlib.Path(__file__).resolve().parents[3]
C = ROOT / "crates/wcore-protocol/contracts/desktop/v1"
core = json.loads((C / "schema/core-event.schema.json").read_text())
prod = json.loads((C / "schema/producer-complete.schema.json").read_text())

out = []


def p(s=""):
    out.append(str(s))


def branches_for(sch, key, disc):
    res = []
    for i, b in enumerate(sch[key]):
        t = b.get("properties", {}).get("type", {})
        en = t.get("enum") or ([t["const"]] if "const" in t else [])
        if disc in en:
            res.append(i)
    return res


p("=== INSTRUMENT CONTROL ===")
tiny = {"type": "object", "required": ["a"]}
p(f"  reject {{}}: {len(list(Draft202012Validator(tiny).iter_errors({})))} err (expect 1)")
p(f"  accept {{'a':1}}: {len(list(Draft202012Validator(tiny).iter_errors({'a':1})))} err (expect 0)")

for disc in ("sub_agent_event", "workflow_started", "workflow_finished", "approval_resume"):
    p()
    p(f"=== {disc} ===")
    ci = branches_for(core, "oneOf", disc)
    pi = branches_for(prod, "anyOf", disc)
    p(f"  core-event branches: {ci}")
    p(f"  producer-complete branches: {pi}")
    for i in ci:
        b = core["oneOf"][i]
        p(f"    core[{i}] required={b.get('required')}")
        p(f"    core[{i}] props={sorted(b.get('properties', {}))}")
    for i in pi:
        b = prod["anyOf"][i]
        p(f"    prod[{i}] required={b.get('required')}")

p()
p("=== corpus fixtures carrying these discriminators ===")
for f in sorted((C / "events").glob("*.json")):
    ev = json.loads(f.read_text())
    if ev.get("type") in ("sub_agent_event", "workflow_started", "workflow_finished", "approval_resume"):
        ci = branches_for(core, "oneOf", ev["type"])
        matches = [i for i in ci if not list(Draft202012Validator(core["oneOf"][i]).iter_errors(ev))]
        p(f"  {f.name}: type={ev['type']} keys={sorted(ev)}")
        p(f"      matches core branches {matches} of candidates {ci} -> {'OK' if len(matches)==1 else 'AMBIGUOUS/UNMATCHED'}")

p()
p("=== Does the LEGACY form of each shared tag have a home in core-event.schema.json? ===")
p("  Rust variant field sets, read from events.rs (source of truth):")
src = (ROOT / "crates/wcore-protocol/src/events.rs").read_text()


def variant_fields(name):
    i = src.index(f"\n    {name} {{")
    j = src.index("\n    },", i)
    body = src[i:j]
    fields = []
    depth = 0
    for line in body.splitlines():
        s = line.strip()
        if s.startswith(("//", "#[")):
            continue
        if depth == 0 and ":" in s and not s.startswith(("///",)):
            fname = s.split(":")[0].strip()
            if fname.isidentifier():
                fields.append(fname)
        depth += s.count("{") - s.count("}")
    return fields


for legacy, correlated in (
    ("SubAgentEvent", "CorrelatedSubAgentEvent"),
    ("WorkflowStarted", "CorrelatedWorkflowStarted"),
    ("WorkflowFinished", "CorrelatedWorkflowFinished"),
):
    lf = variant_fields(legacy)
    cf = variant_fields(correlated)
    p(f"  {legacy}: {lf}")
    p(f"  {correlated}: {cf}")
    p(f"    legacy-only fields: {sorted(set(lf) - set(cf))}")
    p(f"    correlated-only fields: {sorted(set(cf) - set(lf))}")

sys.stdout.write("\n".join(out) + "\n")
