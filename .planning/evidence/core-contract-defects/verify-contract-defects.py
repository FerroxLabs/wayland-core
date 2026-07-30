#!/usr/bin/env python3
"""Lane core-contract-defects: independent verification of C1-C5 at base."""
import json
import pathlib
import sys

from jsonschema import Draft202012Validator

ROOT = pathlib.Path(__file__).parent
C = ROOT / "crates/wcore-protocol/contracts/desktop/v1"

out = []


def p(s=""):
    out.append(str(s))


core = json.loads((C / "schema/core-event.schema.json").read_text())
prod = json.loads((C / "schema/producer-complete.schema.json").read_text())

p("=== INSTRUMENT CONTROL: validator must be able to REJECT and ACCEPT ===")
tiny = {"type": "object", "required": ["a"], "properties": {"a": {"type": "integer"}}}
v = Draft202012Validator(tiny)
p(f"  known-negative (missing 'a'): errors={len(list(v.iter_errors({})))}  (expect >0)")
p(f"  known-positive ({{'a':1}}):     errors={len(list(v.iter_errors({'a': 1})))}  (expect 0)")

p()
p("=== C5: full corpus sweep against core-event.schema.json ===")
cv = Draft202012Validator(core)
pv = Draft202012Validator(prod)
events = sorted((C / "events").glob("*.json"))
p(f"  corpus event files: {len(events)}")
rejected_core, rejected_prod = [], []
for f in events:
    ev = json.loads(f.read_text())
    ec = list(cv.iter_errors(ev))
    ep = list(pv.iter_errors(ev))
    if ec:
        rejected_core.append((f.name, len(ec), [list(e.schema_path) for e in ec]))
    if ep:
        rejected_prod.append((f.name, len(ep), [list(e.schema_path) for e in ep]))
p(f"  REJECTED by core-event.schema.json:      {len(rejected_core)}")
for n, k, sp in rejected_core:
    p(f"    {n}: {k} error(s) schemaPath={sp}")
p(f"  REJECTED by producer-complete.schema.json: {len(rejected_prod)}")
for n, k, sp in rejected_prod:
    p(f"    {n}: {k} error(s) schemaPath={sp}")

p()
p("=== C5 root cause isolation ===")
p(f"  core-event.schema.json top-level oneOf branches: {len(core.get('oneOf', []))}")
p(f"  producer-complete.schema.json top-level anyOf branches: {len(prod.get('anyOf', []))}")

gs = json.loads((C / "events/goal_snapshot.json").read_text())


def find_branch(branches, discriminator):
    hits = []
    for i, b in enumerate(branches):
        t = b.get("properties", {}).get("type", {})
        if t.get("const") == discriminator or discriminator in (t.get("enum") or []):
            hits.append(i)
    return hits


core_gs = find_branch(core["oneOf"], "goal_snapshot")
prod_gs = find_branch(prod["anyOf"], "goal_snapshot")
p(f"  core-event branches with type=goal_snapshot: {core_gs}")
p(f"  producer-complete branches with type=goal_snapshot: {prod_gs}")

for idx in core_gs:
    bv = Draft202012Validator(core["oneOf"][idx])
    errs = list(bv.iter_errors(gs))
    p(f"  core-event.oneOf[{idx}] vs goal_snapshot.json: {len(errs)} error(s)")
    for e in errs:
        p(f"      instancePath=/{'/'.join(map(str, e.absolute_path))} schemaPath={list(e.schema_path)}")

p()
p("=== C5: the tasks/items oneOf itself ===")
for label, sch, idxs in (("core-event", core, core_gs), ("producer-complete", prod, prod_gs)):
    for idx in idxs:
        br = (sch.get("oneOf") or sch.get("anyOf"))[idx]
        try:
            items = br["properties"]["goal"]["properties"]["tasks"]["items"]
        except KeyError:
            p(f"  {label}[{idx}]: no goal/tasks/items path")
            continue
        sub = items.get("oneOf")
        p(f"  {label}[{idx}] required={br.get('required')}")
        if sub is None:
            p(f"  {label}[{idx}] tasks/items has NO oneOf (keys={sorted(items)})")
            continue
        p(f"  {label}[{idx}] tasks/items.oneOf branch count: {len(sub)}")
        for j, sb in enumerate(sub):
            p(
                f"      branch[{j}] title={sb.get('title')!r} "
                f"required={sb.get('required')} "
                f"additionalProperties={sb.get('additionalProperties')} "
                f"propkeys={sorted(sb.get('properties', {}))}"
            )
        # machine-checked membership
        probes = {"{}": {}}
        for k, t in enumerate(gs.get("goal", {}).get("tasks", [])):
            probes[f"task[{k}] id={t.get('id')}"] = t
        for name, probe in probes.items():
            matching = [j for j, sb in enumerate(sub) if not list(Draft202012Validator(sb).iter_errors(probe))]
            p(f"      probe {name} matches branches {matching} -> oneOf {'SATISFIED' if len(matching)==1 else 'VIOLATED'}")

p()
p("=== C3: duplicate discriminators ===")
for label, sch, key in (("core-event", core, "oneOf"), ("producer-complete", prod, "anyOf")):
    seen = {}
    for i, b in enumerate(sch[key]):
        t = b.get("properties", {}).get("type", {})
        names = []
        if "const" in t:
            names = [t["const"]]
        elif "enum" in t:
            names = list(t["enum"])
        for n in names:
            seen.setdefault(n, []).append(i)
    dups = {n: ix for n, ix in seen.items() if len(ix) > 1}
    p(f"  {label}: {len(seen)} distinct discriminators, duplicates={json.dumps(dups)}")

p()
p("=== C4: workspace_policy presence ===")
man = json.loads((C / "manifest.json").read_text())
names = [e["type"] if isinstance(e, dict) else e for e in man["events"]]
p(f"  manifest.json events: {len(names)}")
p(f"  'workspace_policy' in manifest events: {'workspace_policy' in names}")
p(f"  'execution_policy' in manifest events (CONTROL): {'execution_policy' in names}")
core_names = set()
for b in core["oneOf"]:
    t = b.get("properties", {}).get("type", {})
    if "const" in t:
        core_names.add(t["const"])
    core_names.update(t.get("enum") or [])
p(f"  'workspace_policy' in core-event.schema.json: {'workspace_policy' in core_names}")
p(f"  'execution_policy' in core-event.schema.json (CONTROL): {'execution_policy' in core_names}")
for i, b in enumerate(prod["anyOf"]):
    t = b.get("properties", {}).get("type", {})
    en = t.get("enum") or ([t["const"]] if "const" in t else [])
    if "workspace_policy" in en:
        p(f"  producer-complete.anyOf[{i}] title={b.get('title')!r} contains workspace_policy; enum={en}")

sys.stdout.write("\n".join(out) + "\n")
